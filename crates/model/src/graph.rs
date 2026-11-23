//! Pipeline graphs as they are written to disk (TOML) and drawn on the canvas.
//!
//! A run is one item flowing from a source along `route` edges. Routers pick one way out,
//! agents may fan out into variants, a review collapses the variants to the one a person picked,
//! actions do the mechanical work (commands, renders, merges, pull requests).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{IssueStatus, Millis, ModelRef, NodeId, PipelineId, RunnableRef, SkillPolicy, SpawnPolicy, ToolGrants};

/// The schema number written to new files; bump when a field changes meaning.
pub const GRAPH_SCHEMA: u32 = 1;

/// The out-port every node has unless its type says otherwise.
pub const PORT_OUT: &str = "out";
/// Router: taken when no rule matches.
pub const PORT_ELSE: &str = "else";
/// Review: a person shipped a variant.
pub const PORT_APPROVED: &str = "approved";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphDef {
    #[serde(default = "schema_now")]
    pub schema: u32,
    pub id: PipelineId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub workdirs: Vec<WorkdirDef>,
    #[serde(default)]
    pub nodes: Vec<NodeDef>,
    #[serde(default)]
    pub edges: Vec<EdgeDef>,
}

fn schema_now() -> u32 {
    GRAPH_SCHEMA
}

impl GraphDef {
    pub fn node(&self, id: &NodeId) -> Option<&NodeDef> {
        self.nodes.iter().find(|node| &node.id == id)
    }

    pub fn workdir(&self, id: &str) -> Option<&WorkdirDef> {
        self.workdirs.iter().find(|workdir| workdir.id == id)
    }

    pub fn edges_from<'a>(&'a self, id: &'a NodeId) -> impl Iterator<Item = &'a EdgeDef> {
        self.edges.iter().filter(move |edge| &edge.from == id)
    }

    pub fn edges_to<'a>(&'a self, id: &'a NodeId) -> impl Iterator<Item = &'a EdgeDef> {
        self.edges.iter().filter(move |edge| &edge.to == id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Agent instances of one run working at the same time.
    #[serde(default = "default_parallel")]
    pub max_parallel: u32,
    /// Spending cap of one run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// How many times a review may send work back before the run stops and asks.
    #[serde(default = "default_loops")]
    pub max_loops: u32,
}

fn default_parallel() -> u32 {
    2
}

fn default_loops() -> u32 {
    5
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_parallel: default_parallel(), budget_usd: None, max_loops: default_loops() }
    }
}

/// A folder a pipeline works in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkdirDef {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// A fresh git worktree per run (and per variant), or the folder as is.
    #[serde(default)]
    pub worktree: bool,
    #[serde(default)]
    pub base_branch: String,
    #[serde(default)]
    pub branch_prefix: String,
    /// Shell command run once in a new worktree (warm-up build, install).
    #[serde(default)]
    pub setup: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeDef {
    pub id: NodeId,
    pub name: String,
    #[serde(default)]
    pub pos: Pos,
    #[serde(flatten)]
    pub kind: NodeKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    Source(SourceNode),
    Router(RouterNode),
    Agent(AgentNode),
    /// Same executor path as an agent; it leads a team and may have spawn templates.
    Orchestrator(AgentNode),
    /// A template attached to an agent by a `spawn` edge. Never scheduled on its own.
    Subagent(SubagentNode),
    File(FileNode),
    Review(ReviewNode),
    Action(ActionNode),
    /// A nested pipeline. Drawn, not executed yet.
    Pipeline(PipelineNode),
}

impl NodeKind {
    /// The word shown on the node and in the add-node palette.
    pub fn label(&self) -> &'static str {
        match self {
            NodeKind::Source(_) => "Source",
            NodeKind::Router(_) => "Router",
            NodeKind::Agent(_) => "Agent",
            NodeKind::Orchestrator(_) => "Orchestrator",
            NodeKind::Subagent(_) => "Sub-agent",
            NodeKind::File(_) => "File",
            NodeKind::Review(_) => "Review",
            NodeKind::Action(_) => "Action",
            NodeKind::Pipeline(_) => "Pipeline",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// An issue attached to the pipeline on the board.
    #[default]
    Issue,
    /// Started by hand or from the CLI.
    Manual,
    /// An autopilot tick.
    Schedule,
    /// Not available yet.
    Jira,
    /// Not available yet.
    Sentry,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceNode {
    #[serde(default)]
    pub source: SourceKind,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RouterNode {
    /// Tried top to bottom; the first match wins. Each rule is an out-port named by its `id`.
    #[serde(default)]
    pub rules: Vec<RouterRule>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RouterRule {
    /// Port name, unique within the router.
    pub id: String,
    /// What the canvas row shows: "Bug ≥High".
    pub label: String,
    /// A condition over the item: `type == "bug" && priority >= "high"`. Empty when `ask` is set.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when: String,
    /// A yes/no question a model answers with read-only tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<AskRule>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskRule {
    pub question: String,
    #[serde(default)]
    pub model: ModelRef,
    /// Workdir the model may read while deciding.
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub max_steps: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentNode {
    /// The library agent this node uses. The fields below override it inside this pipeline only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runnable: Option<RunnableRef>,
    /// Role text; replaces the library agent's when not empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Id of the workdir it works in.
    #[serde(default)]
    pub workdir: String,
    /// Parallel attempts, each in its own worktree and branch. A person picks one in a review.
    #[serde(default = "one")]
    pub variants: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolGrants>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<SpawnPolicy>,
    /// Spending cap per item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    /// Files it must leave behind, relative to the workdir; checked before the node counts as done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_out: Vec<String>,
}

fn one() -> u32 {
    1
}

/// What an agent may create through its `spawn_agent` tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubagentNode {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub model: ModelRef,
    #[serde(default)]
    pub tools: ToolGrants,
    #[serde(default)]
    pub skills: SkillPolicy,
    #[serde(default)]
    pub max_steps: u32,
    /// How many of these one parent run may create.
    #[serde(default = "one")]
    pub per_run: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    /// Files from a workdir offered to the connected agents as context.
    #[default]
    Read,
    /// An artifact an agent writes and a review shows.
    Write,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileNode {
    #[serde(default)]
    pub workdir: String,
    /// A glob for `read`, a path template for `write` (`out/{key}/spec.md`).
    pub path: String,
    #[serde(default)]
    pub mode: FileMode,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewNode {
    /// The node "Send back" restarts. Must be upstream of the review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_back_to: Option<NodeId>,
    /// Pass straight through when there is nothing to look at (no diff, no render).
    #[serde(default)]
    pub skip_if_empty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActionNode {
    /// A shell command in a workdir.
    Command {
        workdir: String,
        command: String,
        /// Its result is the "Tests" row of the review.
        #[serde(default)]
        tests: bool,
        /// How to read pass/fail counts from the output: `cargo-test`, or a regex with
        /// `passed` and `total` groups. Empty shows pass or fail only.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        parse: String,
        #[serde(default)]
        timeout_secs: u64,
    },
    /// Renders changed HTML files of a workdir to before/after images.
    Render { workdir: String, files: String },
    /// Merges the run's branch of a worktree into its base branch.
    Merge { workdir: String },
    /// Commits, pushes and opens a pull request; posts it to the Inbox.
    PullRequest {
        workdir: String,
        #[serde(default)]
        draft: bool,
        #[serde(default = "yes")]
        push: bool,
    },
    /// Posts a summary of the run to the Inbox.
    Report,
    /// Moves the issue and optionally comments on it.
    UpdateIssue {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<IssueStatus>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        comment: String,
    },
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineNode {
    pub pipeline: PipelineId,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// The item moves along it.
    #[default]
    Route,
    /// A file node feeding an agent.
    Context,
    /// An agent and a sub-agent template it may create.
    Spawn,
    /// A review sending work back upstream. The only edges allowed to close a cycle.
    Back,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeDef {
    pub from: NodeId,
    /// Out-port of `from`: `out`, a router rule id, `else`, `approved`.
    #[serde(default = "port_out")]
    pub port: String,
    pub to: NodeId,
    #[serde(default)]
    pub kind: EdgeKind,
}

fn port_out() -> String {
    PORT_OUT.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionState {
    /// New runs use it.
    Live,
    /// Replaced by a newer deploy; runs that started on it finish on it.
    Retired,
}

/// A deployed snapshot of a pipeline file. The file itself is always the draft.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineVersion {
    pub pipeline: PipelineId,
    pub version: u32,
    pub state: VersionState,
    /// The definition text as deployed.
    pub def_text: String,
    pub hash: String,
    pub note: String,
    pub deployed_at: Millis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// One line of the editor's Problems tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeId>,
    pub message: String,
}

/// One line of "Changes since v14" in the deploy dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum GraphChange {
    Added { node: NodeId, name: String, what: String },
    Removed { node: NodeId, name: String },
    Changed { node: NodeId, name: String, what: String },
    /// Edges, workdirs or limits.
    Wiring { what: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GraphDef {
        GraphDef {
            schema: GRAPH_SCHEMA,
            id: PipelineId::new("demo"),
            name: "Demo".into(),
            description: String::new(),
            limits: Limits::default(),
            workdirs: vec![WorkdirDef { id: "code".into(), name: "Code".into(), path: "/repo".into(), worktree: true, ..Default::default() }],
            nodes: vec![
                NodeDef { id: "src".into(), name: "Issues".into(), pos: Pos { x: 20.0, y: 170.0 }, kind: NodeKind::Source(SourceNode::default()) },
                NodeDef {
                    id: "triage".into(),
                    name: "Triage".into(),
                    pos: Pos::default(),
                    kind: NodeKind::Router(RouterNode {
                        rules: vec![RouterRule {
                            id: "mockup".into(),
                            label: "Needs a mockup".into(),
                            when: String::new(),
                            ask: Some(AskRule { question: "Does it change the UI?".into(), ..Default::default() }),
                        }],
                    }),
                },
                NodeDef {
                    id: "build".into(),
                    name: "Build lead".into(),
                    pos: Pos::default(),
                    kind: NodeKind::Orchestrator(AgentNode { workdir: "code".into(), variants: 1, ..Default::default() }),
                },
                NodeDef {
                    id: "pr".into(),
                    name: "Open PR".into(),
                    pos: Pos::default(),
                    kind: NodeKind::Action(ActionNode::PullRequest { workdir: "code".into(), draft: true, push: true }),
                },
            ],
            edges: vec![
                EdgeDef { from: "src".into(), port: PORT_OUT.into(), to: "triage".into(), kind: EdgeKind::Route },
                EdgeDef { from: "triage".into(), port: PORT_ELSE.into(), to: "build".into(), kind: EdgeKind::Route },
                EdgeDef { from: "build".into(), port: PORT_OUT.into(), to: "pr".into(), kind: EdgeKind::Route },
            ],
        }
    }

    #[test]
    fn a_graph_round_trips_through_toml() {
        let graph = sample();
        let text = toml::to_string_pretty(&graph).unwrap();
        assert!(text.contains("type = \"router\""), "{text}");
        assert!(text.contains("action = \"pull_request\""), "{text}");
        let back: GraphDef = toml::from_str(&text).unwrap();
        assert_eq!(back, graph);
    }

    #[test]
    fn a_graph_round_trips_through_json() {
        let graph = sample();
        let back: GraphDef = serde_json::from_str(&serde_json::to_string(&graph).unwrap()).unwrap();
        assert_eq!(back, graph);
    }

    #[test]
    fn a_minimal_file_gets_defaults() {
        let graph: GraphDef = toml::from_str(
            r#"
            id = "tiny"
            name = "Tiny"
            [[nodes]]
            id = "a"
            name = "Coder"
            type = "agent"
            [[edges]]
            from = "a"
            to = "a"
            kind = "back"
            "#,
        )
        .unwrap();
        assert_eq!(graph.schema, GRAPH_SCHEMA);
        assert_eq!(graph.limits.max_parallel, 2);
        let NodeKind::Agent(agent) = &graph.nodes[0].kind else { panic!("agent expected") };
        assert_eq!(agent.variants, 1);
        assert_eq!(graph.edges[0].port, PORT_OUT);
        assert_eq!(graph.edges_from(&"a".into()).count(), 1);
    }
}
