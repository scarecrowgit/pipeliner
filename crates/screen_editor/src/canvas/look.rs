//! What a node shows on the canvas, derived from its kind: icon, tone, sub-line, rows and the
//! ports it offers. Pure functions of the graph; nothing here draws.

use gpui::SharedString;
use hub::model::{
    ActionNode, AgentNode, EdgeKind, FileMode, GraphDef, NodeDef, NodeKind, PORT_APPROVED, PORT_ELSE, PORT_OUT,
    SourceKind, ToolAccess,
};
use ui::assets::IconName;
use ui::components::TileTone;
use ui_rich::canvas::Outputs;

/// The out-port of a review that sends work back upstream. The model names no constant for
/// it; the fixture and the engine's back edges use this name.
pub const PORT_SEND_BACK: &str = "send_back";

/// Everything the canvas draws for one node, apart from where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeLook {
    pub icon: IconName,
    /// The tile tone when the node is not selected (selected nodes use the accent).
    pub tone: TileTone,
    pub subtitle: SharedString,
    pub rows: Vec<(SharedString, SharedString)>,
    /// Has an in-port at the left of the header.
    pub input: bool,
    pub outputs: Outputs,
    /// Out-port names, top to bottom, one per output position of `outputs`.
    pub ports: Vec<String>,
    /// Drawn as created by another agent: dashed violet.
    pub spawned: bool,
    /// Not available yet: dimmed, "Soon".
    pub soon: bool,
}

fn s(text: impl Into<SharedString>) -> SharedString {
    text.into()
}

fn row(key: &str, value: impl Into<SharedString>) -> (SharedString, SharedString) {
    (s(key.to_string()), value.into())
}

/// The name of node `id`, or the id itself when the node is missing.
fn name_of(graph: &GraphDef, id: &hub::model::NodeId) -> String {
    graph.node(id).map_or_else(|| id.to_string(), |node| node.name.clone())
}

/// What a workdir is called on a row.
fn workdir_name(graph: &GraphDef, id: &str) -> String {
    match graph.workdir(id) {
        Some(workdir) if !workdir.name.is_empty() => workdir.name.clone(),
        Some(_) => id.to_string(),
        None if id.is_empty() => "no folder".to_string(),
        None => id.to_string(),
    }
}

/// Tools of a level, as the level's own docs list them (`ToolAccess`): read_file, list_dir and
/// grep; plus bash; plus edit and write.
fn level_tools(access: ToolAccess) -> usize {
    match access {
        ToolAccess::ReadOnly => 3,
        ToolAccess::ReadAndRun => 4,
        ToolAccess::Full => 6,
    }
}

fn agent_look(agent: &AgentNode, orchestrator: bool) -> NodeLook {
    let model = agent.model.as_ref().filter(|m| !m.is_empty()).map_or("library model", |m| m.short());
    let tools = match &agent.tools {
        Some(grants) => {
            let count = (level_tools(grants.access) + grants.allow.len()).saturating_sub(grants.deny.len());
            count.to_string()
        }
        None => "library".to_string(),
    };
    let spawns = match &agent.spawn {
        Some(spawn) if spawn.enabled => format!("depth ≤{}", spawn.max_depth),
        Some(_) => "off".to_string(),
        None => "library".to_string(),
    };
    NodeLook {
        icon: if orchestrator { IconName::Orchestrator } else { IconName::Agent },
        tone: TileTone::Neutral,
        subtitle: s(format!("{model} · ×{}", agent.variants.max(1))),
        rows: vec![row("tools", tools), row("spawns", spawns)],
        input: true,
        outputs: Outputs::One,
        ports: vec![PORT_OUT.to_string()],
        spawned: false,
        soon: false,
    }
}

/// The target of the first edge leaving `node` through `port`.
fn target_of(graph: &GraphDef, node: &NodeDef, port: &str) -> Option<String> {
    graph.edges_from(&node.id).find(|edge| edge.port == port).map(|edge| name_of(graph, &edge.to))
}

fn action_look(graph: &GraphDef, action: &ActionNode) -> (String, Vec<(SharedString, SharedString)>) {
    let base_of = |workdir: &str| {
        graph.workdir(workdir).map(|w| w.base_branch.clone()).filter(|b| !b.is_empty()).unwrap_or_else(|| "base".into())
    };
    match action {
        ActionNode::Command { workdir, command, tests, .. } => {
            let sub = if command.trim().is_empty() { "no command yet".to_string() } else { command.clone() };
            let mut rows = vec![row("runs in", workdir_name(graph, workdir))];
            if *tests {
                rows.push(row("counts as", "tests"));
            }
            (sub, rows)
        }
        ActionNode::Render { workdir, files } => {
            let sub = if files.trim().is_empty() { "changed HTML".to_string() } else { files.clone() };
            (sub, vec![row("renders in", workdir_name(graph, workdir))])
        }
        ActionNode::Merge { workdir } => (format!("branch → {}", base_of(workdir)), vec![row("repo", workdir_name(graph, workdir))]),
        ActionNode::PullRequest { workdir, draft, .. } => {
            let prefix = graph.workdir(workdir).map(|w| w.branch_prefix.clone()).unwrap_or_default();
            let mut rows = vec![row("repo", workdir_name(graph, workdir))];
            if *draft {
                rows.push(row("draft", "yes"));
            }
            (format!("{prefix}{{key}} → {}", base_of(workdir)), rows)
        }
        ActionNode::Report => ("summary to the Inbox".to_string(), vec![row("posts to", "Inbox")]),
        ActionNode::UpdateIssue { status, comment } => {
            let sub = match status {
                Some(status) => format!("status → {}", status.label()),
                None => "comment only".to_string(),
            };
            let comment = if comment.trim().is_empty() { "none".to_string() } else { comment.clone() };
            (sub, vec![row("comment", comment)])
        }
    }
}

/// What `node` shows, in `graph` (other nodes give names to "by", "else" and "on approve").
pub fn look(graph: &GraphDef, node: &NodeDef) -> NodeLook {
    let header = |icon, tone, subtitle: String, rows| NodeLook {
        icon,
        tone,
        subtitle: s(subtitle),
        rows,
        input: true,
        outputs: Outputs::One,
        ports: vec![PORT_OUT.to_string()],
        spawned: false,
        soon: false,
    };
    match &node.kind {
        NodeKind::Source(source) => {
            let (sub, word) = match source.source {
                SourceKind::Issue => ("issues on the board", "issue"),
                SourceKind::Manual => ("started by hand", "manual"),
                SourceKind::Schedule => ("on a schedule", "schedule"),
                SourceKind::Jira => ("Jira issues", "jira"),
                SourceKind::Sentry => ("Sentry errors", "sentry"),
            };
            let mut rows = vec![row("pulls", word)];
            if matches!(source.source, SourceKind::Jira | SourceKind::Sentry) {
                rows.push(row("status", "Coming soon"));
            }
            NodeLook { input: false, ..header(IconName::Source, TileTone::Info, sub.to_string(), rows) }
        }
        NodeKind::Router(router) => {
            let mut rows: Vec<_> = router
                .rules
                .iter()
                .enumerate()
                .map(|(i, rule)| {
                    let label = if rule.label.is_empty() { rule.id.clone() } else { rule.label.clone() };
                    row(&format!("rule {}", i + 1), label)
                })
                .collect();
            rows.push(row("else", target_of(graph, node, PORT_ELSE).unwrap_or_else(|| "skip".into())));
            let mut ports: Vec<String> = router.rules.iter().map(|rule| rule.id.clone()).collect();
            ports.push(PORT_ELSE.to_string());
            NodeLook {
                outputs: Outputs::PerRow,
                ports,
                ..header(IconName::Router, TileTone::Neutral, "first match wins".into(), rows)
            }
        }
        NodeKind::Agent(agent) => agent_look(agent, false),
        NodeKind::Orchestrator(agent) => agent_look(agent, true),
        NodeKind::Subagent(sub) => {
            let parent = graph
                .edges_to(&node.id)
                .find(|edge| edge.kind == EdgeKind::Spawn)
                .map(|edge| name_of(graph, &edge.from));
            let subtitle = parent.as_ref().map_or_else(|| "not attached".to_string(), |p| format!("by {p}"));
            NodeLook {
                outputs: Outputs::None,
                ports: Vec::new(),
                spawned: true,
                ..header(
                    IconName::Spawn,
                    TileTone::Spawn,
                    subtitle,
                    vec![row("per run", format!("≤{}", sub.per_run)), row("returns to", parent.unwrap_or_else(|| "—".into()))],
                )
            }
        }
        NodeKind::File(file) => {
            let sub = if file.path.trim().is_empty() { "no path yet".to_string() } else { file.path.clone() };
            let (mode, input) = match file.mode {
                FileMode::Read => ("read", false),
                FileMode::Write => ("write", true),
            };
            NodeLook {
                input,
                ..header(
                    IconName::File,
                    TileTone::Neutral,
                    sub,
                    vec![row("mode", mode), row("in", workdir_name(graph, &file.workdir))],
                )
            }
        }
        NodeKind::Review(review) => {
            let approve = target_of(graph, node, PORT_APPROVED).unwrap_or_else(|| "end".into());
            let back = review.send_back_to.as_ref().map_or_else(|| "off".to_string(), |id| name_of(graph, id));
            NodeLook {
                outputs: Outputs::PerRow,
                ports: vec![PORT_APPROVED.to_string(), PORT_SEND_BACK.to_string()],
                ..header(
                    IconName::Inbox,
                    TileTone::Neutral,
                    "ship 1 of N".into(),
                    vec![row("on approve", approve), row("on send back", back)],
                )
            }
        }
        NodeKind::Action(action) => {
            let (sub, rows) = action_look(graph, action);
            header(IconName::Zap, TileTone::Neutral, sub, rows)
        }
        NodeKind::Pipeline(pipeline) => NodeLook {
            soon: true,
            ..header(IconName::Pipelines, TileTone::Neutral, pipeline.pipeline.to_string(), vec![row("status", "Coming soon")])
        },
    }
}
