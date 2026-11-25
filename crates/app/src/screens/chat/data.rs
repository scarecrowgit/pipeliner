//! Store queries for the chat screen. Everything here runs on a background thread
//! (through `screens::load`), never inside `render`.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use harness_core::{
    AgentDef, Card, Chat, ChatMode, Config, Decision, DecisionKind, Id, Message, NodeKind,
    Pipeline, Role, Run, Status, Store, Task,
};

/// One row of the chat list.
#[derive(Clone)]
pub struct ChatRow {
    pub chat: Chat,
    /// Status of the chat's own task (None before the first message).
    pub status: Option<Status>,
    /// When the task entered its current state (for "waiting 6m").
    pub since: Option<DateTime<Utc>>,
    pub cost: f64,
    /// Agent name, pipeline name or short model.
    pub detail: String,
    /// Latest run started from this chat: number and status.
    pub run: Option<(u64, Status)>,
}

impl ChatRow {
    /// The status that drives the row's dot: an active run wins over an idle chat task.
    pub fn live_status(&self) -> Option<Status> {
        match (self.status, self.run) {
            (Some(s), _) if s.is_active() => Some(s),
            (_, Some((_, r))) if r.is_active() => Some(r),
            _ => None,
        }
    }
}

/// All chats with what the list shows about each.
pub fn chat_rows(store: &Store) -> Vec<ChatRow> {
    let chats = store.chats();
    let agents: HashMap<Id, String> = store.agents().into_iter().map(|a| (a.id, a.name)).collect();
    let pipelines: HashMap<Id, String> = store
        .pipelines()
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut latest_run: HashMap<Id, (u64, Status)> = HashMap::new();
    for r in store.runs() {
        if let Some(c) = r.chat_id {
            let e = latest_run.entry(c).or_insert((r.number, r.status));
            if r.number >= e.0 {
                *e = (r.number, r.status);
            }
        }
    }
    chats
        .into_iter()
        .map(|chat| {
            let task = chat.task_id.as_deref().and_then(|t| store.task(t));
            let detail = match chat.mode {
                ChatMode::Agent => chat
                    .agent_id
                    .as_ref()
                    .and_then(|a| agents.get(a).cloned())
                    .unwrap_or_default(),
                ChatMode::Pipeline => chat
                    .pipeline_id
                    .as_ref()
                    .and_then(|p| pipelines.get(p).cloned())
                    .unwrap_or_default(),
                _ => short_model(&chat.model),
            };
            ChatRow {
                status: task.as_ref().map(|t| t.status),
                since: task.as_ref().and_then(|t| t.started_at),
                cost: task.as_ref().map(|t| t.cost_usd).unwrap_or(0.0),
                run: latest_run.get(&chat.id).copied(),
                detail,
                chat,
            }
        })
        .collect()
}

/// One node of a run as a chip in the run card.
#[derive(Clone)]
pub struct NodeChip {
    pub name: String,
    /// `None`: not reached yet.
    pub status: Option<Status>,
    /// The (latest) task that executed this node, to open its thread.
    pub task_id: Option<Id>,
}

/// A run started from this chat, with what its card shows.
#[derive(Clone)]
pub struct RunInfo {
    pub run: Run,
    pub pipeline: String,
    pub nodes: Vec<NodeChip>,
    /// A task of this run that waits for you (gate).
    pub waiting_task: Option<Id>,
}

impl RunInfo {
    pub fn done_nodes(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.status.is_some_and(|s| s.is_finished()))
            .count()
    }
}

/// Everything about the open chat except its messages.
#[derive(Clone)]
pub struct Detail {
    pub chat: Chat,
    pub task: Option<Task>,
    /// Sub-tasks the chat's task delegated (mini-board).
    pub children: Vec<Task>,
    pub runs: HashMap<Id, RunInfo>,
    /// Decisions on the chat's task, oldest first (plan approvals).
    pub decisions: Vec<Decision>,
    /// Whether `chat.cwd` exists (None when unset).
    pub cwd_valid: Option<bool>,
}

impl Detail {
    /// Task ids whose changes should refresh this detail.
    pub fn task_ids(&self) -> HashSet<Id> {
        let mut s: HashSet<Id> = self.children.iter().map(|t| t.id.clone()).collect();
        if let Some(t) = &self.task {
            s.insert(t.id.clone());
        }
        for r in self.runs.values() {
            s.extend(r.nodes.iter().filter_map(|n| n.task_id.clone()));
        }
        s
    }
    /// The most recent active run, for the header badge.
    pub fn active_run(&self) -> Option<&RunInfo> {
        self.runs
            .values()
            .filter(|r| r.run.status.is_active())
            .max_by_key(|r| r.run.number)
    }
}

pub fn detail(store: &Store, chat_id: &str) -> Option<Detail> {
    let chat = store.chat(chat_id)?;
    let task = chat.task_id.as_deref().and_then(|t| store.task(t));
    let mut children = task
        .as_ref()
        .map(|t| store.children(&t.id))
        .unwrap_or_default();
    children.sort_by_key(|x| x.created_at);
    let mut decisions = task
        .as_ref()
        .map(|t| store.decisions_for(&t.id))
        .unwrap_or_default();
    decisions.sort_by_key(|x| x.created_at);
    let run_ids: HashSet<Id> = store
        .messages(chat_id)
        .into_iter()
        .filter_map(|m| match m.card {
            Some(Card::Run { run_id }) => Some(run_id),
            _ => None,
        })
        .collect();
    let runs = run_ids
        .into_iter()
        .filter_map(|id| run_info(store, &id).map(|r| (id, r)))
        .collect();
    let cwd_valid = chat
        .cwd
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(cwd_exists);
    Some(Detail {
        chat,
        task,
        children,
        runs,
        decisions,
        cwd_valid,
    })
}

/// Whether a working folder exists (`~` expanded).
pub fn cwd_exists(path: &str) -> bool {
    harness_core::config::expand_tilde(path.trim()).is_dir()
}

fn run_info(store: &Store, run_id: &str) -> Option<RunInfo> {
    let run = store.run(run_id)?;
    let pipeline = store
        .pipeline(&run.pipeline_id)
        .map(|p| p.name)
        .unwrap_or_else(|| "Pipeline".into());
    let tasks = store.tasks_for_run(run_id);
    let mut by_node: HashMap<&str, Vec<&Task>> = HashMap::new();
    for t in &tasks {
        if let Some(n) = t.node_id.as_deref() {
            by_node.entry(n).or_default().push(t);
        }
    }
    let nodes = graph_order(&run)
        .into_iter()
        .filter(|n| !matches!(n.kind, NodeKind::Source(_)))
        .map(|n| {
            let ts = by_node.get(n.id.as_str()).cloned().unwrap_or_default();
            let status = if ts.is_empty() {
                if run.outputs.contains_key(&n.id) {
                    Some(Status::Done)
                } else if run.frontier.contains(&n.id) && run.status.is_active() {
                    Some(Status::Queued)
                } else {
                    None
                }
            } else {
                Some(aggregate(ts.iter().map(|t| t.status)))
            };
            let task_id = ts.iter().max_by_key(|t| t.created_at).map(|t| t.id.clone());
            NodeChip {
                name: n.name.clone(),
                status,
                task_id,
            }
        })
        .collect();
    let waiting_task = tasks
        .iter()
        .find(|t| t.status == Status::Waiting)
        .map(|t| t.id.clone());
    Some(RunInfo {
        run,
        pipeline,
        nodes,
        waiting_task,
    })
}

/// One status for a node that ran several tasks (fan-out variants, retries).
fn aggregate(statuses: impl Iterator<Item = Status>) -> Status {
    let v: Vec<Status> = statuses.collect();
    for s in [Status::Waiting, Status::Running, Status::Queued] {
        if v.contains(&s) {
            return s;
        }
    }
    if v.iter().all(|s| *s == Status::Done) {
        Status::Done
    } else if v.contains(&Status::Failed) {
        Status::Failed
    } else {
        v.last().copied().unwrap_or(Status::Cancelled)
    }
}

/// Nodes in flow order (breadth first from the roots), then any unreachable ones.
fn graph_order(run: &Run) -> Vec<&harness_core::Node> {
    let g = &run.graph;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = vec![];
    let mut queue: VecDeque<&harness_core::Node> = g.roots().into_iter().collect();
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n.id.as_str()) {
            continue;
        }
        out.push(n);
        for e in g.next(&n.id, None) {
            if let Some(m) = g.node(&e.to) {
                queue.push_back(m);
            }
        }
    }
    out.extend(g.nodes.iter().filter(|n| !seen.contains(n.id.as_str())));
    out
}

/// Messages of a thread, oldest first.
pub fn messages(store: &Store, chat_id: &str) -> Vec<Message> {
    let mut v = store.messages(chat_id);
    v.sort_by_key(|x| x.created_at);
    v
}

/// What the pickers offer.
#[derive(Clone, Default)]
pub struct Pickers {
    pub models: Vec<String>,
    pub agents: Vec<AgentDef>,
    pub pipelines: Vec<Pipeline>,
}

/// Agents and (live) pipelines. Models come from the engine separately (may hit the network).
pub fn pickers(store: &Store) -> (Vec<AgentDef>, Vec<Pipeline>) {
    let mut agents = store.agents();
    agents.sort_by_key(|a| a.name.to_lowercase());
    let mut pipelines: Vec<Pipeline> = store
        .pipelines()
        .into_iter()
        .filter(|p| !p.archived)
        .collect();
    pipelines.sort_by_key(|p| p.name.to_lowercase());
    (agents, pipelines)
}

// ---------------------------------------------------------------------------
// Plan cards
// ---------------------------------------------------------------------------

/// Where a plan card stands.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanState {
    /// The latest plan and the task waits for your decision.
    Pending,
    Approved(DateTime<Utc>),
    Feedback(DateTime<Utc>),
    Rejected(DateTime<Utc>),
    /// Superseded by a newer plan or the task ended without a decision.
    Closed,
}

/// Matches each plan card to the first decision taken after it (and before the next plan).
pub fn plan_states(
    messages: &[Message],
    decisions: &[Decision],
    task_status: Option<Status>,
) -> HashMap<Id, PlanState> {
    let plans: Vec<&Message> = messages
        .iter()
        .filter(|m| matches!(m.card, Some(Card::Plan { .. })))
        .collect();
    let mut out = HashMap::new();
    for (i, m) in plans.iter().enumerate() {
        let next = plans.get(i + 1).map(|n| n.created_at);
        let d = decisions
            .iter()
            .find(|d| d.created_at >= m.created_at && next.is_none_or(|n| d.created_at < n));
        let state = match d.map(|d| (&d.kind, d.created_at)) {
            Some((DecisionKind::Approve | DecisionKind::Pick { .. }, t)) => PlanState::Approved(t),
            Some((DecisionKind::Reply { .. } | DecisionKind::SendBack { .. }, t)) => {
                PlanState::Feedback(t)
            }
            Some((DecisionKind::Reject { .. }, t)) => PlanState::Rejected(t),
            None if next.is_none() && task_status == Some(Status::Waiting) => PlanState::Pending,
            None => PlanState::Closed,
        };
        out.insert(m.id.clone(), state);
    }
    out
}

/// Index of `Role::Tool` results by the tool call id they answer.
pub fn tool_results(messages: &[Message]) -> HashMap<String, usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::Tool)
        .filter_map(|(i, m)| m.tool_call_id.clone().map(|id| (id, i)))
        .collect()
}

pub fn short_model(m: &str) -> String {
    if m.is_empty() {
        String::new()
    } else {
        Config::short_model(m)
    }
}

/// One line describing a tool call's arguments (`path`, `command`, ...).
pub fn args_summary(arguments: &str) -> String {
    let line = match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(serde_json::Value::Object(map)) => {
            const KEYS: [&str; 9] = [
                "command", "path", "pattern", "query", "title", "name", "file", "url", "steps",
            ];
            let pick = KEYS
                .iter()
                .find_map(|k| map.get(*k))
                .or_else(|| map.values().next());
            match pick {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Array(a)) => format!("{} items", a.len()),
                Some(v) => v.to_string(),
                None => String::new(),
            }
        }
        _ => arguments.to_string(),
    };
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 90 {
        format!("{}…", line.chars().take(89).collect::<String>())
    } else {
        line
    }
}

/// Pretty-printed JSON arguments (raw text when not JSON).
pub fn pretty_args(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| arguments.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::{now, Message, Role};

    fn plan_msg(at: DateTime<Utc>) -> Message {
        let mut m = Message::new("c", Role::System, "Plan ready");
        m.card = Some(Card::Plan {
            task_id: "t".into(),
            steps: vec!["a".into()],
        });
        m.created_at = at;
        m
    }

    fn decision(kind: DecisionKind, at: DateTime<Utc>) -> Decision {
        Decision {
            id: harness_core::new_id(),
            task_id: "t".into(),
            run_id: None,
            kind,
            created_at: at,
            consumed: true,
        }
    }

    #[test]
    fn plan_cards_match_their_decisions() {
        let t0 = now();
        let p1 = plan_msg(t0);
        let p2 = plan_msg(t0 + chrono::Duration::seconds(10));
        let d1 = decision(
            DecisionKind::Reply { text: "x".into() },
            t0 + chrono::Duration::seconds(5),
        );
        let s = plan_states(&[p1.clone(), p2.clone()], &[d1], Some(Status::Waiting));
        assert!(matches!(s[&p1.id], PlanState::Feedback(_)));
        assert_eq!(s[&p2.id], PlanState::Pending);
        let s = plan_states(&[p1.clone(), p2.clone()], &[], Some(Status::Done));
        assert_eq!(s[&p2.id], PlanState::Closed);
    }

    #[test]
    fn summarises_tool_args() {
        assert_eq!(
            args_summary(r#"{"path":"src/main.rs","x":1}"#),
            "src/main.rs"
        );
        assert_eq!(
            args_summary(r#"{"command":"cargo  test\n--all"}"#),
            "cargo test --all"
        );
        assert_eq!(args_summary("not json"), "not json");
    }
}
