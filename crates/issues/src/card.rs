//! What a board card says about the latest run of its issue.

use base::fmt;
use model::{Issue, IssueStatus, NodeInstance, NodeStatus, Run, RunState, SpanKind};

#[cfg(test)]
mod tests;

/// The status line of a card, derived from the issue's latest run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardState {
    /// Nothing is going on.
    Idle,
    /// About to start: "Queued", "Starts soon".
    Starting { text: String },
    /// Work in progress: "Reproducer running", plus the agent it created, plus progress.
    Running {
        text: String,
        /// Name of the agent another agent created, drawn in violet: "Log digger".
        spawned: Option<String>,
        /// Percent of the run's node instances finished; `None` before any exists.
        percent: Option<u32>,
    },
    /// Waits for a person: "3 variants to review", "asks: which log sinks are in scope?".
    Waiting { text: String },
    /// Finished: "PR #2280 merged".
    Done { text: String },
    /// Failed or interrupted.
    Failed { text: String },
}

/// The state of `issue`'s card. `run` is the issue's latest run, looked up by the caller.
pub fn card_state(issue: &Issue, run: Option<&Run>) -> CardState {
    let Some(run) = run else {
        let starts = issue.status == IssueStatus::Todo
            && issue.runner.is_some()
            && (issue.triggers.on_assign || issue.triggers.on_todo);
        return if starts { CardState::Starting { text: "Starts soon".into() } } else { CardState::Idle };
    };
    match run.state {
        RunState::Queued => CardState::Starting { text: "Queued".into() },
        RunState::Running => running(run),
        RunState::WaitingReview => CardState::Waiting { text: review_text(run) },
        RunState::WaitingAnswer => CardState::Waiting { text: question_text(run) },
        RunState::Done => CardState::Done { text: done_text(issue, run) },
        RunState::Failed => {
            let error = run.error.as_deref().map(fmt::first_line).unwrap_or("");
            let text = if error.is_empty() { "Failed".to_string() } else { format!("Failed · {}", fmt::truncate_chars(error, 80)) };
            CardState::Failed { text }
        }
        RunState::Interrupted => CardState::Failed { text: "Interrupted · resume it".into() },
        RunState::Cancelled => CardState::Idle,
    }
}

fn is_spawned(n: &NodeInstance) -> bool {
    n.kind == SpanKind::Spawned || n.spawned_by.is_some()
}

fn running(run: &Run) -> CardState {
    let active: Vec<&NodeInstance> = run.nodes.iter().filter(|n| n.status == NodeStatus::Running).collect();
    let main: Vec<&&NodeInstance> = active.iter().filter(|n| !is_spawned(n)).collect();
    let text = match main.as_slice() {
        [] => "Running".to_string(),
        [one] => format!("{} running", one.name),
        many if many.iter().all(|n| n.name == many[0].name) => format!("{} ×{} running", many[0].name, many.len()),
        many => format!("{} agents running", many.len()),
    };
    let spawned = active.iter().rev().find(|n| is_spawned(n)).map(|n| n.name.clone());
    let percent = (!run.nodes.is_empty()).then(|| fmt::percent(f64::from(run.progress()), 1.0));
    CardState::Running { text, spawned, percent }
}

fn review_text(run: &Run) -> String {
    let variants = run
        .nodes
        .iter()
        .filter(|n| n.status == NodeStatus::Waiting && n.variant > 0)
        .count() as u64;
    if variants > 0 { format!("{} to review", fmt::plural(variants, "variant", "variants")) } else { "Ready to review".into() }
}

/// The question is the summary of the node that waits; runs carry no separate question field.
fn question_text(run: &Run) -> String {
    let question = run
        .nodes
        .iter()
        .rev()
        .filter(|n| n.status == NodeStatus::Waiting)
        .map(|n| fmt::first_line(&n.summary))
        .find(|s| !s.is_empty());
    match question {
        Some(q) => format!("asks: {}", fmt::truncate_chars(q, 80)),
        None => "asks you a question".into(),
    }
}

fn done_text(issue: &Issue, run: &Run) -> String {
    let Some(url) = run.pr_url.as_ref().or(issue.pr_url.as_ref()) else {
        return "Finished".into();
    };
    let label = match pr_number(url) {
        Some(n) => format!("PR #{n}"),
        None => "PR".into(),
    };
    let state = if issue.status == IssueStatus::Done { "merged" } else { "open" };
    format!("{label} {state}")
}

/// The number at the end of a pull request URL: `.../pull/2280` gives 2280.
pub fn pr_number(url: &str) -> Option<u64> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}
