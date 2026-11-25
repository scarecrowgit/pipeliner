//! Domain model. Everything that runs is a [`Task`]; every change is an [`Event`].
//!
//! Top row (what you design): [`AgentDef`], [`Pipeline`] (+ [`Node`], [`Edge`], [`Workdir`]).
//! Middle row (what runs): [`Chat`], [`Run`], [`Task`], [`Variant`], [`Decision`].
//! Bottom row (what it leaves behind): [`Message`], [`Artifact`], [`Event`].

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type Id = String;

pub fn new_id() -> Id {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

pub fn now() -> DateTime<Utc> {
    Utc::now()
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

/// Which tools an agent may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolAccess {
    /// No tools: answers from the prompt only.
    None,
    /// read_file, list_dir, search, load_skill, read_workdir.
    ReadOnly,
    /// ReadOnly + run_command (no file writes).
    ReadAndRun,
    /// Everything: also write_file, edit_file.
    #[default]
    Full,
}

impl ToolAccess {
    pub const ALL: [ToolAccess; 4] = [Self::None, Self::ReadOnly, Self::ReadAndRun, Self::Full];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "No tools",
            Self::ReadOnly => "Read only",
            Self::ReadAndRun => "Read and run",
            Self::Full => "Full",
        }
    }
}

/// A saved agent: role prompt (layer 2), model, tools and skills.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDef {
    pub id: Id,
    pub name: String,
    /// One line shown in lists.
    #[serde(default)]
    pub description: String,
    /// Prompt layer 2: who the agent is and how it works.
    #[serde(default)]
    pub role: String,
    /// `provider/model`, e.g. `nr/cc/claude-sonnet-5-5`.
    pub model: String,
    #[serde(default)]
    pub tools: ToolAccess,
    /// Skills always loaded into the prompt.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Skills the agent may load with `load_skill`.
    #[serde(default)]
    pub skills_on_demand: Vec<String>,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    #[serde(default = "one")]
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn default_max_steps() -> u32 {
    40
}
fn one() -> u32 {
    1
}

// ---------------------------------------------------------------------------
// Pipelines
// ---------------------------------------------------------------------------

/// A folder agents work in. With `worktree`, every run gets its own git worktree.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workdir {
    pub id: Id,
    pub name: String,
    pub path: String,
    #[serde(default = "yes")]
    pub worktree: bool,
    #[serde(default = "main_branch")]
    pub base_branch: String,
    /// Branch name prefix for run branches, e.g. `harness/`.
    #[serde(default)]
    pub branch_prefix: String,
    /// Shell command run once after the worktree is created (e.g. a warm-up build).
    #[serde(default)]
    pub setup: String,
    /// Wraps every command agents run here, e.g. `nix-shell shell.nix --run`.
    /// The command is appended as one quoted argument.
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}
fn main_branch() -> String {
    "main".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pipeline {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Sidebar dot color (0xRRGGBB).
    #[serde(default = "default_color")]
    pub color: u32,
    /// Incremented on every deploy. Runs record the version they used.
    #[serde(default = "one")]
    pub version: u32,
    /// Unsaved edits live in `draft`; `Deploy` copies them over the live graph.
    #[serde(default)]
    pub draft: Option<PipelineGraph>,
    pub graph: PipelineGraph,
    #[serde(default)]
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_color() -> u32 {
    0x4ea7fc
}

impl Pipeline {
    /// The graph the editor shows: the draft if there is one, else the live graph.
    pub fn editing(&self) -> &PipelineGraph {
        self.draft.as_ref().unwrap_or(&self.graph)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct PipelineGraph {
    #[serde(default)]
    pub workdirs: Vec<Workdir>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

impl PipelineGraph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    pub fn workdir(&self, id: &str) -> Option<&Workdir> {
        self.workdirs.iter().find(|w| w.id == id)
    }
    /// Nodes with no incoming edge (normally the single source).
    pub fn roots(&self) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|n| !self.edges.iter().any(|e| e.to == n.id))
            .collect()
    }
    /// Outgoing edges of `id`; when `port` is given only edges with that port.
    pub fn next(&self, id: &str, port: Option<&str>) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|e| e.from == id && port.map_or(true, |p| e.port.as_deref() == Some(p)))
            .collect()
    }
    pub fn incoming(&self, id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.to == id).collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: Id,
    pub name: String,
    pub kind: NodeKind,
    /// Canvas position (editor only).
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    /// Where a run's input comes from. Every run starts here.
    Source(SourceNode),
    /// An LLM answers a yes/no question about the input. Out ports: `yes`, `no`.
    Decision(DecisionNode),
    /// Rules on run fields, top to bottom, first match wins. Out ports: `rule1..N`, `else`.
    Router(RouterNode),
    /// An agent works on the task. With `variants > 1` it fans out.
    Agent(AgentNode),
    /// Renders HTML files changed in a workdir to PNG for review.
    Render(RenderNode),
    /// Waits for you: pick a variant, approve, or send back.
    Gate(GateNode),
    /// A deterministic step: shell command, pull request, merge.
    Tool(ToolNode),
}

impl NodeKind {
    pub fn label(&self) -> &'static str {
        match self {
            NodeKind::Source(_) => "Source",
            NodeKind::Decision(_) => "Decision",
            NodeKind::Router(_) => "Router",
            NodeKind::Agent(_) => "Agent step",
            NodeKind::Render(_) => "Render",
            NodeKind::Gate(_) => "Gate",
            NodeKind::Tool(_) => "Tool step",
        }
    }
    /// Output ports. `None` entry means the default (unnamed) port.
    pub fn ports(&self) -> Vec<Option<String>> {
        match self {
            NodeKind::Decision(_) => vec![Some("yes".into()), Some("no".into())],
            NodeKind::Router(r) => {
                let mut v: Vec<Option<String>> =
                    (1..=r.rules.len()).map(|i| Some(format!("rule{i}"))).collect();
                v.push(Some("else".into()));
                v
            }
            _ => vec![None],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceNode {
    /// What the input is called in prompts, e.g. "issue".
    #[serde(default = "issue")]
    pub input_name: String,
    /// Optional GitHub repo `owner/name` to read issues from (`gh issue view`).
    #[serde(default)]
    pub github_repo: String,
}
fn issue() -> String {
    "issue".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct DecisionNode {
    pub question: String,
    pub model: String,
    /// Workdir the model may read (read-only tools) to decide.
    #[serde(default)]
    pub workdir: Option<Id>,
    #[serde(default = "twelve")]
    pub max_steps: u32,
}
fn twelve() -> u32 {
    12
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct RouterNode {
    pub rules: Vec<RouterRule>,
}

/// `field op value`, e.g. `input contains "crash"`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RouterRule {
    /// `input`, `title`, or `out.<node_id>` (an upstream node's summary).
    pub field: String,
    pub op: RuleOp,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleOp {
    Contains,
    Equals,
    StartsWith,
    Matches,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentNode {
    /// Saved agent to use (role = layer 2). When `None`, `model`/`tools` below apply.
    #[serde(default)]
    pub agent: Option<Id>,
    /// Overrides the agent's model when non-empty.
    #[serde(default)]
    pub model: String,
    /// Prompt layer 3: what this step must do.
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub workdir: Option<Id>,
    #[serde(default)]
    pub tools: Option<ToolAccess>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub skills_on_demand: Vec<String>,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    /// Fan-out: number of independent variants (each its own task and worktree).
    #[serde(default = "one")]
    pub variants: u32,
    /// Optional per-variant strategy hints, appended to instructions.
    #[serde(default)]
    pub variant_hints: Vec<String>,
    /// Lets this agent hand sub-tasks to a coder sub-agent with the `delegate` tool.
    #[serde(default)]
    pub delegate: Option<DelegateConfig>,
}

impl Default for AgentNode {
    fn default() -> Self {
        Self {
            agent: None,
            model: String::new(),
            instructions: String::new(),
            workdir: None,
            tools: None,
            skills: vec![],
            skills_on_demand: vec![],
            max_steps: default_max_steps(),
            variants: 1,
            variant_hints: vec![],
            delegate: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DelegateConfig {
    pub name: String,
    pub model: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub skills_on_demand: Vec<String>,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RenderNode {
    pub workdir: Option<Id>,
    /// Glob of files to render when changed, relative to the workdir.
    #[serde(default = "html_glob")]
    pub files: String,
    /// Render command; `{in}` and `{out}` are replaced. Empty = built-in chromium.
    #[serde(default)]
    pub command: String,
    #[serde(default = "w1440")]
    pub width: u32,
    #[serde(default = "h1600")]
    pub height: u32,
}

impl Default for RenderNode {
    fn default() -> Self {
        Self { workdir: None, files: html_glob(), command: String::new(), width: 1440, height: 1600 }
    }
}
fn html_glob() -> String {
    "**/*.html".into()
}
fn w1440() -> u32 {
    1440
}
fn h1600() -> u32 {
    1600
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// Pick one of the upstream variants (or approve the only one).
    #[default]
    Pick,
    /// Approve the run's pull request; approving merges it.
    ApprovePr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct GateNode {
    #[serde(default)]
    pub kind: GateKind,
    /// Shown as the review item title.
    #[serde(default)]
    pub prompt: String,
    /// Node a send-back re-runs (with your note as prompt layer 6).
    #[serde(default)]
    pub back_to: Option<Id>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolNode {
    /// Runs a shell command in a workdir. Fails the run on non-zero exit unless allowed.
    Command { workdir: Option<Id>, command: String, #[serde(default)] allow_failure: bool },
    /// Commits the workdir's run branch, pushes it and opens a pull request with `gh`.
    PullRequest { workdir: Option<Id>, #[serde(default)] draft: bool },
    /// Merges the run's pull request with `gh pr merge`.
    MergePr { #[serde(default = "squash")] method: String },
}
fn squash() -> String {
    "squash".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub from: Id,
    pub to: Id,
    /// Output port of `from` (`yes`/`no`, `rule1`, `else`), `None` = default.
    #[serde(default)]
    pub port: Option<String>,
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Running,
    /// Waiting for you (a gate, a plan approval, a question).
    Waiting,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Queued => "Queued",
            Status::Running => "Running",
            Status::Waiting => "Waiting for you",
            Status::Done => "Done",
            Status::Failed => "Failed",
            Status::Cancelled => "Cancelled",
        }
    }
    pub fn is_active(self) -> bool {
        matches!(self, Status::Queued | Status::Running | Status::Waiting)
    }
    pub fn is_finished(self) -> bool {
        !self.is_active()
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Waiting => "waiting",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }
}

/// One execution of a pipeline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: Id,
    /// Human number, `#12`.
    pub number: u64,
    pub pipeline_id: Id,
    pub pipeline_version: u32,
    /// Snapshot of the graph this run executes (later edits don't affect it).
    pub graph: PipelineGraph,
    pub title: String,
    /// The source item (issue text). Prompt layer 4.
    pub input: String,
    #[serde(default)]
    pub external_ref: Option<String>,
    pub status: Status,
    /// Chat that started the run, if any; the result posts back there.
    #[serde(default)]
    pub chat_id: Option<Id>,
    /// Per-node outputs: node id -> summary/value. Filled as nodes finish.
    #[serde(default)]
    pub outputs: BTreeMap<Id, NodeOutput>,
    /// Nodes ready to execute (resumable frontier).
    #[serde(default)]
    pub frontier: Vec<Id>,
    /// Per workdir: the branch that holds this run's current state.
    #[serde(default)]
    pub heads: BTreeMap<Id, String>,
    /// Per workdir: the worktree path for single-variant steps.
    #[serde(default)]
    pub worktrees: BTreeMap<Id, String>,
    /// Send-back notes waiting for the node they target (node id -> note).
    #[serde(default)]
    pub notes: BTreeMap<Id, String>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct NodeOutput {
    pub summary: String,
    /// Port taken (decision/router).
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub value: serde_json::Value,
    pub task_id: Option<Id>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// A chat conversation (direct, agent, orchestrator).
    Chat,
    Agent,
    Decision,
    Router,
    Render,
    Gate,
    Tool,
    /// Sub-task made by an agent (`delegate`).
    Spawned,
    Source,
}

impl TaskKind {
    pub fn label(self) -> &'static str {
        match self {
            TaskKind::Chat => "Chat",
            TaskKind::Agent => "Agent",
            TaskKind::Decision => "Decision",
            TaskKind::Router => "Router",
            TaskKind::Render => "Render",
            TaskKind::Gate => "Gate",
            TaskKind::Tool => "Tool",
            TaskKind::Spawned => "Sub-agent",
            TaskKind::Source => "Source",
        }
    }
}

/// One unit of work: a card on the Board, a bar on the Timeline, a thread you can open.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: Id,
    pub kind: TaskKind,
    pub title: String,
    pub status: Status,
    #[serde(default)]
    pub run_id: Option<Id>,
    #[serde(default)]
    pub node_id: Option<Id>,
    #[serde(default)]
    pub chat_id: Option<Id>,
    /// Parent task for spawned sub-agents and fan-out variants.
    #[serde(default)]
    pub parent_id: Option<Id>,
    /// Spawn depth (0 for pipeline steps).
    #[serde(default)]
    pub depth: u32,
    /// Variant index for fan-out (0 = A).
    #[serde(default)]
    pub variant: Option<u32>,
    /// Who works on it: agent name, "you", or a tool name.
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub model: String,
    /// Worktree / folder this task works in.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    /// Lower sorts first on the Board and Timeline.
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub output: serde_json::Value,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
}

impl Task {
    pub fn new(kind: TaskKind, title: impl Into<String>) -> Self {
        Task {
            id: new_id(),
            kind,
            title: title.into(),
            status: Status::Queued,
            run_id: None,
            node_id: None,
            chat_id: None,
            parent_id: None,
            depth: 0,
            variant: None,
            actor: String::new(),
            model: String::new(),
            cwd: None,
            branch: None,
            priority: 0,
            summary: String::new(),
            output: serde_json::Value::Null,
            cost_usd: 0.0,
            tokens_in: 0,
            tokens_out: 0,
            steps: 0,
            error: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        }
    }
}

pub fn variant_letter(i: u32) -> char {
    (b'A' + (i % 26) as u8) as char
}

/// One of N outputs a gate shows side by side.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    pub id: Id,
    /// The gate task this variant is offered in.
    pub gate_task_id: Id,
    pub run_id: Option<Id>,
    /// The task that produced it.
    pub source_task_id: Option<Id>,
    pub index: u32,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub duration_secs: f64,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Unified diff against the run's head before this step.
    #[serde(default)]
    pub diff: String,
    /// Rendered images (absolute paths) for design variants.
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub picked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecisionKind {
    Pick { index: u32 },
    Approve,
    SendBack { note: String, #[serde(default)] target: Option<Id> },
    Reject { #[serde(default)] reason: String },
    /// Answer to an agent's question (or plan feedback).
    Reply { text: String },
}

/// What you decided at a gate. Also the source for "Decided today".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub id: Id,
    pub task_id: Id,
    pub run_id: Option<Id>,
    pub kind: DecisionKind,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub consumed: bool,
}

// ---------------------------------------------------------------------------
// Chats and messages
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatMode {
    /// You and a model.
    #[default]
    Direct,
    /// A saved agent with its role and tools.
    Agent,
    /// Writes a plan, you approve it, it delegates parts to sub-agents.
    Orchestrator,
    /// Starts a saved pipeline with the message as input.
    Pipeline,
}

impl ChatMode {
    pub const ALL: [ChatMode; 4] = [Self::Direct, Self::Agent, Self::Orchestrator, Self::Pipeline];
    pub fn label(self) -> &'static str {
        match self {
            ChatMode::Direct => "Direct",
            ChatMode::Agent => "Agent",
            ChatMode::Orchestrator => "Orchestrator",
            ChatMode::Pipeline => "Pipeline",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chat {
    pub id: Id,
    pub title: String,
    pub mode: ChatMode,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub agent_id: Option<Id>,
    #[serde(default)]
    pub pipeline_id: Option<Id>,
    /// Folder the agent/orchestrator works in.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The chat's own task (shows on Board/Timeline while it works).
    #[serde(default)]
    pub task_id: Option<Id>,
    #[serde(default)]
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    /// Tool call result.
    Tool,
    /// Engine notes: step started, spawned a task, run finished.
    System,
}

/// A message in a chat or a task thread (same format in both).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: Id,
    /// Chat id or task id.
    pub thread_id: Id,
    pub role: Role,
    #[serde(default)]
    pub author: String,
    pub content: String,
    /// Assistant tool calls (OpenAI format) or the tool name for Role::Tool.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// Inline card attached to the message (plan, run progress, variant picker...).
    #[serde(default)]
    pub card: Option<Card>,
    /// Still streaming.
    #[serde(default)]
    pub partial: bool,
    /// For user messages posted mid-run: has the agent seen it yet.
    #[serde(default)]
    pub delivered: bool,
    pub created_at: DateTime<Utc>,
}

impl Message {
    pub fn new(thread_id: impl Into<Id>, role: Role, content: impl Into<String>) -> Self {
        Message {
            id: new_id(),
            thread_id: thread_id.into(),
            role,
            author: String::new(),
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            card: None,
            partial: false,
            delivered: false,
            created_at: now(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Card {
    /// Orchestrator plan waiting for approval (task id of the approval).
    Plan { task_id: Id, steps: Vec<String> },
    /// Live progress of a run started from this chat.
    Run { run_id: Id },
    /// Sub-tasks spawned from this chat (mini-board).
    SubTasks { parent_task_id: Id },
    /// A task this message refers to.
    Task { task_id: Id },
}

/// A picked output kept after a run: PR, branch, file, text.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Id,
    pub run_id: Option<Id>,
    pub task_id: Option<Id>,
    pub kind: ArtifactKind,
    pub title: String,
    /// URL, path or text.
    pub value: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    PullRequest,
    Branch,
    File,
    Image,
    Text,
}

/// Append-only event log entry. Every view is a query on state that events describe.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub ts: DateTime<Utc>,
    pub kind: String,
    #[serde(default)]
    pub run_id: Option<Id>,
    #[serde(default)]
    pub task_id: Option<Id>,
    pub text: String,
}
