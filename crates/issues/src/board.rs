//! View rules of the issue board and list: columns, counts, filter chips and sorting.

use std::cmp::Ordering;

use model::{Issue, IssueStatus, Run, RunId, RunnableKind, RunnableRef};

use crate::keys;

#[cfg(test)]
mod tests;

/// Looks a run up by id. Passed by reference so the board never clones a run.
pub type RunLookup<'r> = dyn Fn(&RunId) -> Option<&'r Run> + 'r;

/// The "Runs with" filter chips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RunsWith {
    #[default]
    All,
    Orchestrators,
    Agents,
    Pipelines,
    Unattached,
}

impl RunsWith {
    pub const ALL: [RunsWith; 5] =
        [RunsWith::All, RunsWith::Orchestrators, RunsWith::Agents, RunsWith::Pipelines, RunsWith::Unattached];

    pub fn label(self) -> &'static str {
        match self {
            RunsWith::All => "All",
            RunsWith::Orchestrators => "Orchestrators",
            RunsWith::Agents => "Agents",
            RunsWith::Pipelines => "Pipelines",
            RunsWith::Unattached => "Unattached",
        }
    }

    /// Whether `issue` passes the filter.
    pub fn accepts(self, issue: &Issue) -> bool {
        self == RunsWith::All || runs_with_of(issue) == self
    }
}

/// The chip an issue counts under: by the kind of its attached runner. Created agents
/// (`spawned:`) count as agents.
fn runs_with_of(issue: &Issue) -> RunsWith {
    match issue.runner.as_ref().map(RunnableRef::scheme) {
        None => RunsWith::Unattached,
        Some("orchestrator") => RunsWith::Orchestrators,
        Some("pipeline") => RunsWith::Pipelines,
        Some(_) => RunsWith::Agents,
    }
}

/// The kind of runner an issue is attached to, if any.
pub fn runner_kind(issue: &Issue) -> Option<RunnableKind> {
    match runs_with_of(issue) {
        RunsWith::Orchestrators => Some(RunnableKind::Orchestrator),
        RunsWith::Pipelines => Some(RunnableKind::Pipeline),
        RunsWith::Agents => Some(RunnableKind::Agent),
        _ => None,
    }
}

/// One filter chip with its count over all issues of the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chip {
    pub filter: RunsWith,
    pub label: &'static str,
    pub count: usize,
}

/// The chips in display order. Orchestrators, Agents, Pipelines and Unattached add up to All.
pub fn chips(issues: &[Issue]) -> Vec<Chip> {
    RunsWith::ALL
        .iter()
        .map(|&filter| Chip { filter, label: filter.label(), count: issues.iter().filter(|i| filter.accepts(i)).count() })
        .collect()
}

/// One board column.
#[derive(Clone, Debug)]
pub struct Column<'a> {
    pub status: IssueStatus,
    /// Priority high first, then most recently updated, then key.
    pub issues: Vec<&'a Issue>,
    /// Issues whose latest run waits for a person.
    pub needs_you: usize,
}

impl Column<'_> {
    pub fn count(&self) -> usize {
        self.issues.len()
    }

    /// "1 needs you", "2 need you"; `None` when nothing waits.
    pub fn needs_label(&self) -> Option<String> {
        needs_label(self.needs_you)
    }
}

/// "1 needs you", "2 need you"; `None` for zero.
pub fn needs_label(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("1 needs you".to_string()),
        n => Some(format!("{n} need you")),
    }
}

/// The five columns and the numbers of the header.
#[derive(Clone, Debug)]
pub struct Board<'a> {
    pub columns: Vec<Column<'a>>,
    /// Issues that are not Done: "8 open".
    pub open: usize,
    pub needs_you: usize,
}

/// Groups `issues` that pass `filter` into the five columns, in a stable order.
pub fn build<'a>(issues: &'a [Issue], filter: RunsWith, runs: &RunLookup<'_>) -> Board<'a> {
    let mut columns: Vec<Column<'a>> =
        IssueStatus::ALL.iter().map(|&status| Column { status, issues: Vec::new(), needs_you: 0 }).collect();
    for issue in issues.iter().filter(|i| filter.accepts(i)) {
        let column = &mut columns[IssueStatus::ALL.iter().position(|s| *s == issue.status).unwrap_or(0)];
        column.needs_you += usize::from(needs_you(issue, runs));
        column.issues.push(issue);
    }
    for column in &mut columns {
        column.issues.sort_by(|a, b| card_order(a, b));
    }
    let open = columns.iter().filter(|c| c.status != IssueStatus::Done).map(Column::count).sum();
    let needs_you = columns.iter().map(|c| c.needs_you).sum();
    Board { columns, open, needs_you }
}

/// Whether the latest run of `issue` waits for a person.
pub fn needs_you(issue: &Issue, runs: &RunLookup<'_>) -> bool {
    issue.run.as_ref().and_then(runs).is_some_and(|run| run.state.needs_person())
}

fn card_order(a: &Issue, b: &Issue) -> Ordering {
    b.priority.cmp(&a.priority).then(b.updated.cmp(&a.updated)).then_with(|| keys::compare(a.key.as_str(), b.key.as_str()))
}

/// A column of the list view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListColumn {
    Key,
    Title,
    Status,
    Priority,
    Updated,
}

impl ListColumn {
    /// The direction a first click on the header sorts in: high priority and recent first.
    pub fn default_dir(self) -> SortDir {
        match self {
            ListColumn::Priority | ListColumn::Updated => SortDir::Desc,
            _ => SortDir::Asc,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    /// A to Z, low to high, old to new, Backlog to Done.
    Asc,
    Desc,
}

/// The issues sorted for the list view. Ties fall back to the key in natural order, whatever
/// the direction, so the order is stable.
pub fn sort_list(issues: &[Issue], by: ListColumn, dir: SortDir) -> Vec<&Issue> {
    let mut rows: Vec<&Issue> = issues.iter().collect();
    rows.sort_by(|a, b| {
        let primary = match by {
            ListColumn::Key => keys::compare(a.key.as_str(), b.key.as_str()),
            ListColumn::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            ListColumn::Status => a.status.cmp(&b.status),
            ListColumn::Priority => a.priority.cmp(&b.priority),
            ListColumn::Updated => a.updated.cmp(&b.updated),
        };
        let primary = if dir == SortDir::Desc { primary.reverse() } else { primary };
        primary.then_with(|| keys::compare(a.key.as_str(), b.key.as_str()))
    });
    rows
}
