//! What the list shows, as plain data: tabs and groups, which runnable goes where, the search,
//! and the words of each row (state, activity, where it lives). No GPUI here, so all of it is
//! tested without a window.

use hub::api::{GraphDef, NodeKind, Origin, Runnable, RunnableActivity, RunnableKind, RunnableRef, RunnableState};

/// A tab of the bar under the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Tab {
    All,
    Orchestrators,
    Agents,
    Pipelines,
    Created,
    /// Shown only while something is archived.
    Archived,
}

impl Tab {
    /// The tabs in bar order.
    pub(crate) const ALL: [Tab; 6] = [Tab::All, Tab::Orchestrators, Tab::Agents, Tab::Pipelines, Tab::Created, Tab::Archived];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Orchestrators => "Orchestrators",
            Tab::Agents => "Agents",
            Tab::Pipelines => "Pipelines",
            Tab::Created => "Created by agents",
            Tab::Archived => "Archived",
        }
    }

    /// The `<name>` of the tab's probe, `agents-tab-<name>`.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Tab::All => "all",
            Tab::Orchestrators => "orchestrators",
            Tab::Agents => "agents",
            Tab::Pipelines => "pipelines",
            Tab::Created => "created",
            Tab::Archived => "archived",
        }
    }

    /// Whether `runnable` is listed under this tab.
    pub(crate) fn admits(self, runnable: &Runnable) -> bool {
        if self == Tab::Archived {
            return runnable.archived;
        }
        if runnable.archived {
            return false;
        }
        match self {
            Tab::All | Tab::Archived => true,
            Tab::Created => is_spawned(runnable),
            other => group_of(runnable).tab() == other,
        }
    }
}

/// A group of the list, in list order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Group {
    Orchestrators,
    Pipelines,
    Agents,
    Created,
}

impl Group {
    pub(crate) const ALL: [Group; 4] = [Group::Orchestrators, Group::Pipelines, Group::Agents, Group::Created];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Group::Orchestrators => "Orchestrators",
            Group::Pipelines => "Pipelines",
            Group::Agents => "Agents",
            Group::Created => "Created by agents",
        }
    }

    fn tab(self) -> Tab {
        match self {
            Group::Orchestrators => Tab::Orchestrators,
            Group::Pipelines => Tab::Pipelines,
            Group::Agents => Tab::Agents,
            Group::Created => Tab::Created,
        }
    }
}

/// An agent another agent created at run time.
pub(crate) fn is_spawned(runnable: &Runnable) -> bool {
    matches!(runnable.origin, Origin::Spawned { .. })
}

/// The group a runnable is listed in.
pub(crate) fn group_of(runnable: &Runnable) -> Group {
    if is_spawned(runnable) {
        return Group::Created;
    }
    match runnable.kind {
        RunnableKind::Orchestrator => Group::Orchestrators,
        RunnableKind::Pipeline => Group::Pipelines,
        RunnableKind::Agent => Group::Agents,
    }
}

/// The search matches name, model or runtime (and the kind word), ignoring case.
pub(crate) fn matches(runnable: &Runnable, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let mut hay = vec![runnable.name.to_lowercase(), kind_word(runnable.kind).to_lowercase()];
    if let Some(agent) = &runnable.agent {
        hay.push(agent.model.to_string().to_lowercase());
        hay.push(agent.runtime.to_lowercase());
    }
    hay.iter().any(|text| text.contains(&query))
}

/// The runnables of `tab` that match `query`, grouped in list order.
pub(crate) fn grouped<'a>(runnables: &'a [Runnable], tab: Tab, query: &str) -> Vec<(Group, Vec<&'a Runnable>)> {
    Group::ALL
        .iter()
        .map(|group| {
            let rows: Vec<&Runnable> = runnables
                .iter()
                .filter(|r| group_of(r) == *group && tab.admits(r) && matches(r, query))
                .collect();
            (*group, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// How many runnables a tab lists, before the search.
pub(crate) fn count(runnables: &[Runnable], tab: Tab) -> usize {
    runnables.iter().filter(|r| tab.admits(r)).count()
}

/// "Agent", "Orchestrator", "Pipeline".
pub(crate) fn kind_word(kind: RunnableKind) -> &'static str {
    match kind {
        RunnableKind::Agent => "Agent",
        RunnableKind::Orchestrator => "Orchestrator",
        RunnableKind::Pipeline => "Pipeline",
    }
}

/// The colour family of a row's state word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Secondary,
    Muted,
    Success,
    Warning,
    Danger,
}

/// The dot a state shows; mirrors `ui::Status` without depending on it here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dot {
    Live,
    Running,
    Waiting,
    Error,
    Draft,
    Idle,
}

/// A state mark: the dot and the words after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StateMark {
    pub dot: Dot,
    pub text: String,
    pub tone: Tone,
}

impl StateMark {
    fn new(dot: Dot, text: impl Into<String>, tone: Tone) -> Self {
        Self { dot, text: text.into(), tone }
    }
}

fn with_issue(word: &str, issue: &Option<hub::model::IssueKey>) -> String {
    match issue {
        Some(key) => format!("{word} · {key}"),
        None => word.to_string(),
    }
}

/// What a row says about what the runnable is doing now.
pub(crate) fn state_mark(runnable: &Runnable, activity: Option<&RunnableActivity>) -> StateMark {
    if runnable.archived {
        return StateMark::new(Dot::Idle, "archived", Tone::Muted);
    }
    let state = activity.map(|a| a.state.clone()).unwrap_or_default();
    if runnable.paused && !matches!(state, RunnableState::Running { .. }) {
        return StateMark::new(Dot::Draft, "paused", Tone::Muted);
    }
    match state {
        RunnableState::Idle => StateMark::new(Dot::Idle, "idle", Tone::Muted),
        RunnableState::Ready => StateMark::new(Dot::Live, "ready", Tone::Success),
        RunnableState::Running { issue } => StateMark::new(Dot::Running, with_issue("running", &issue), Tone::Secondary),
        RunnableState::Waiting { issue } => StateMark::new(Dot::Waiting, with_issue("asks you", &issue), Tone::Warning),
        RunnableState::Queued { count } => StateMark::new(Dot::Draft, format!("{count} queued"), Tone::Secondary),
        RunnableState::Paused => StateMark::new(Dot::Draft, "paused", Tone::Muted),
        RunnableState::Draft { version } => StateMark::new(Dot::Draft, format!("draft v{version}"), Tone::Muted),
        RunnableState::Live { version } => StateMark::new(Dot::Live, format!("v{version} live"), Tone::Success),
        RunnableState::Alive { issue } => StateMark::new(Dot::Running, with_issue("alive", &issue), Tone::Secondary),
        RunnableState::Ended { times } if times > 1 => StateMark::new(Dot::Idle, format!("ended · ×{times}"), Tone::Muted),
        RunnableState::Ended { .. } => StateMark::new(Dot::Idle, "ended", Tone::Muted),
    }
}

/// The issue the runnable works on right now, if any.
pub(crate) fn running_issue(activity: Option<&RunnableActivity>) -> Option<hub::model::IssueKey> {
    match activity.map(|a| &a.state) {
        Some(RunnableState::Running { issue } | RunnableState::Alive { issue } | RunnableState::Waiting { issue }) => {
            issue.clone()
        }
        _ => None,
    }
}

/// "14 runs today · $1.12".
pub(crate) fn runs_today(activity: Option<&RunnableActivity>) -> String {
    let (runs, cost) = activity.map_or((0, 0.0), |a| (a.runs_today, a.cost_today_usd));
    format!("{} today · {}", base::fmt::plural(u64::from(runs), "run", "runs"), base::fmt::cost(cost))
}

/// Agents and orchestrators a pipeline graph runs.
pub(crate) fn agents_inside(graph: &GraphDef) -> usize {
    graph.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Agent(_) | NodeKind::Orchestrator(_))).count()
}

/// The Activity column: numbers for orchestrators and pipelines, model and runtime for agents,
/// spend for created agents.
pub(crate) fn activity_text(runnable: &Runnable, activity: Option<&RunnableActivity>, graph: Option<&GraphDef>) -> String {
    if is_spawned(runnable) {
        let cost = base::fmt::cost(activity.map_or(0.0, |a| a.cost_today_usd));
        return match activity.map(|a| &a.state) {
            Some(RunnableState::Alive { .. }) => format!("{cost} so far"),
            _ => cost,
        };
    }
    match runnable.kind {
        RunnableKind::Orchestrator => runs_today(activity),
        RunnableKind::Pipeline => {
            if matches!(activity.map(|a| &a.state), Some(RunnableState::Draft { .. })) {
                return "not deployed".to_string();
            }
            let mut parts = Vec::new();
            let queued = activity.map_or(0, |a| a.queued);
            if queued > 0 {
                parts.push(format!("{} queued", base::fmt::count(u64::from(queued))));
            }
            if let Some(graph) = graph {
                let n = agents_inside(graph) as u64;
                parts.push(format!("{} inside", base::fmt::plural(n, "agent", "agents")));
            }
            if parts.is_empty() { runs_today(activity) } else { parts.join(" · ") }
        }
        RunnableKind::Agent => {
            let Some(agent) = &runnable.agent else { return String::new() };
            let mut parts = Vec::new();
            if !agent.model.is_empty() {
                parts.push(agent.model.short().to_string());
            }
            if !agent.runtime.is_empty() {
                parts.push(agent.runtime.clone());
            }
            parts.join(" · ")
        }
    }
}

/// The small tag after the kind for definitions that do not live in the library.
pub(crate) fn origin_tag(origin: &Origin) -> Option<&'static str> {
    match origin {
        Origin::Builtin => Some("Built-in"),
        Origin::Project { .. } => Some("Project"),
        Origin::ClaudeCode { .. } => Some("Claude Code"),
        Origin::Library | Origin::Spawned { .. } => None,
    }
}

/// Why a runnable cannot be changed here, if it cannot: the tooltip of disabled menu items
/// and the note on read-only panels.
pub(crate) fn read_only_reason(runnable: &Runnable) -> Option<&'static str> {
    match runnable.origin {
        Origin::Builtin => Some("Built in · duplicate it to make your own"),
        Origin::ClaudeCode { .. } => Some("A Claude Code agent file · read-only here"),
        Origin::Spawned { .. } => Some("Created at run time · duplicate it to keep it"),
        Origin::Library | Origin::Project { .. } => None,
    }
}

/// Why a runnable cannot be deleted or archived, if it cannot.
pub(crate) fn removal_blocked(runnable: &Runnable) -> Option<&'static str> {
    match runnable.origin {
        Origin::Builtin => Some("Built-in agents can't be removed"),
        Origin::ClaudeCode { .. } => Some("Remove the file in Claude Code instead"),
        _ => None,
    }
}

/// The runnable a list position stands for, in the order the list draws them.
pub(crate) fn order(groups: &[(Group, Vec<&Runnable>)]) -> Vec<RunnableRef> {
    groups.iter().flat_map(|(_, rows)| rows.iter().map(|r| r.reference.clone())).collect()
}

/// The reference `step` rows away from `from` in `order` (clamped at the ends); the first row
/// when nothing is selected or the selection is not listed.
pub(crate) fn step(order: &[RunnableRef], from: Option<&RunnableRef>, step: isize) -> Option<RunnableRef> {
    if order.is_empty() {
        return None;
    }
    let Some(at) = from.and_then(|f| order.iter().position(|r| r == f)) else {
        return order.first().cloned();
    };
    let to = (at as isize + step).clamp(0, order.len() as isize - 1) as usize;
    order.get(to).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hub::api::{AgentDef, ModelRef, Uses};

    fn runnable(reference: RunnableRef, kind: RunnableKind, name: &str, origin: Origin) -> Runnable {
        Runnable {
            reference,
            kind,
            name: name.into(),
            description: String::new(),
            origin,
            uses: Uses::default(),
            paused: false,
            archived: false,
            agent: Some(AgentDef { model: ModelRef::new("cc", "opus-5.5"), runtime: "cloud".into(), ..AgentDef::default() }),
            path: None,
        }
    }

    fn library() -> Vec<Runnable> {
        let lead = RunnableRef::orchestrator("lead");
        vec![
            runnable(lead.clone(), RunnableKind::Orchestrator, "Lead", Origin::Library),
            Runnable { agent: None, ..runnable(RunnableRef::pipeline("triage"), RunnableKind::Pipeline, "Triage", Origin::Library) },
            runnable(RunnableRef::agent("build"), RunnableKind::Agent, "Build", Origin::Builtin),
            runnable(RunnableRef::spawned("helper"), RunnableKind::Agent, "Helper", Origin::Spawned { by: lead, run: None, session: None }),
            Runnable { archived: true, ..runnable(RunnableRef::agent("old"), RunnableKind::Agent, "Old", Origin::Library) },
        ]
    }

    #[test]
    fn tabs_and_groups_split_the_library() {
        let all = library();
        assert_eq!(count(&all, Tab::All), 4);
        assert_eq!(count(&all, Tab::Agents), 1, "created agents have their own tab");
        assert_eq!(count(&all, Tab::Created), 1);
        assert_eq!(count(&all, Tab::Archived), 1);
        let groups: Vec<Group> = grouped(&all, Tab::All, "").into_iter().map(|(g, _)| g).collect();
        assert_eq!(groups, Group::ALL);
    }

    #[test]
    fn the_search_looks_at_name_model_and_runtime() {
        let all = library();
        assert_eq!(order(&grouped(&all, Tab::All, "LEAD")), [RunnableRef::orchestrator("lead")]);
        assert_eq!(order(&grouped(&all, Tab::Agents, "opus")), [RunnableRef::agent("build")]);
        assert_eq!(order(&grouped(&all, Tab::All, "cloud")).len(), 3, "the pipeline has no runtime");
        assert!(grouped(&all, Tab::All, "nothing like it").is_empty());
    }

    #[test]
    fn states_read_like_the_mockup() {
        let r = runnable(RunnableRef::agent("a"), RunnableKind::Agent, "A", Origin::Library);
        let act = |state| RunnableActivity { state, ..RunnableActivity::default() };
        let running = act(RunnableState::Running { issue: Some(hub::model::IssueKey::new("PAY-1")) });
        assert_eq!(state_mark(&r, Some(&running)).text, "running · PAY-1");
        assert_eq!(state_mark(&r, Some(&act(RunnableState::Live { version: 14 }))).text, "v14 live");
        assert_eq!(state_mark(&r, Some(&act(RunnableState::Ended { times: 2 }))).text, "ended · ×2");
        assert_eq!(state_mark(&Runnable { paused: true, ..r.clone() }, None).text, "paused");
        assert_eq!(state_mark(&Runnable { archived: true, ..r }, None).text, "archived");
    }

    #[test]
    fn activity_differs_by_kind() {
        let all = library();
        let a = RunnableActivity { runs_today: 14, cost_today_usd: 1.12, ..RunnableActivity::default() };
        assert_eq!(activity_text(&all[0], Some(&a), None), "14 runs today · $1.12");
        assert_eq!(activity_text(&all[2], None, None), "opus-5.5 · cloud");
        let alive = RunnableActivity { state: RunnableState::Alive { issue: None }, cost_today_usd: 0.08, ..a };
        assert_eq!(activity_text(&all[3], Some(&alive), None), "$0.08 so far");
    }

    #[test]
    fn steps_clamp_at_the_ends() {
        let order = [RunnableRef::agent("a"), RunnableRef::agent("b")];
        assert_eq!(step(&order, None, 1), Some(order[0].clone()));
        assert_eq!(step(&order, Some(&order[0]), 1), Some(order[1].clone()));
        assert_eq!(step(&order, Some(&order[1]), 1), Some(order[1].clone()));
        assert_eq!(step(&order, Some(&order[1]), -1), Some(order[0].clone()));
    }
}
