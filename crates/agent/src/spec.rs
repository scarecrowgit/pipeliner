//! The arguments and the result of [`crate::run`]: what the agent is, where it runs, what it
//! may call, what it is asked, and why it stopped.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use base::time::Clock;
use catalog::Catalog;
use llm::{LlmClient, Reasoning};
use model::{
    AgentDef, Attachment, ChatMode, FileChange, ModelRef, NodeId, RunId, RunnableRef, SessionId, SkillPolicy,
    SpawnPolicy, ToolGrants, Usage,
};
use serde_json::{Value, json};
use tools::ToolRegistry;

use crate::gate::PermissionGate;
use crate::host::Host;
use crate::mcp::Mcp;

/// Model calls per [`crate::run`] when the spec says 0.
pub const DEFAULT_MAX_STEPS: u32 = 50;

/// What the agent is. The same spec runs in chat, as a pipeline node and as a sub-agent; the
/// differences are in these fields.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentSpec {
    /// Display name: "Build", "Fixer".
    pub name: String,
    /// Who spends, for the ledger.
    pub runnable: RunnableRef,
    /// The body of the system prompt.
    pub role: String,
    pub model: ModelRef,
    pub reasoning: Reasoning,
    /// The built-in level, plus `allow` and `deny` by name. Both lists take a trailing `*`
    /// (`mcp__github__*`). MCP tools are offered only when `allow` names them; session tools
    /// (`todo_write`, `ask_user`…) are offered unless `deny` names them.
    pub tools: ToolGrants,
    pub skills: SkillPolicy,
    /// Whether `spawn_agent` is offered, and its limits.
    pub spawn: SpawnPolicy,
    /// The kinds of sub-agent it may create by name, besides the general-purpose one.
    pub templates: Vec<SubagentTemplate>,
    /// Stops the run once the session and its sub-agents have spent this much. Every
    /// sub-agent below checks it too, before each of its model calls, and is given no more
    /// than what is left of it.
    pub budget_usd: Option<f64>,
    /// Model calls per run; 0 means [`DEFAULT_MAX_STEPS`].
    pub max_steps: u32,
    pub finish: Finish,
    pub limits: Limits,
}

impl AgentSpec {
    /// A spec from a library definition, finishing by reply (as chat does) with default limits.
    pub fn from_def(name: impl Into<String>, runnable: RunnableRef, def: &AgentDef) -> Self {
        Self {
            name: name.into(),
            runnable,
            role: def.role.clone(),
            model: def.model.clone(),
            reasoning: Reasoning::Default,
            tools: def.tools.clone(),
            skills: def.skills.clone(),
            spawn: def.spawn.clone(),
            templates: Vec::new(),
            budget_usd: def.budget_usd,
            max_steps: def.max_steps,
            finish: Finish::Reply,
            limits: Limits::default(),
        }
    }

    /// Model calls allowed in one run.
    pub fn step_limit(&self) -> u32 {
        if self.max_steps == 0 { DEFAULT_MAX_STEPS } else { self.max_steps }
    }
}

/// A kind of sub-agent the model may create with `spawn_agent`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubagentTemplate {
    /// What the model passes as `agent`.
    pub name: String,
    /// One line telling the model when to use it.
    pub description: String,
    pub role: String,
    /// `None` uses the parent's model.
    pub model: Option<ModelRef>,
    /// Narrowed to what the parent itself has.
    pub tools: ToolGrants,
    /// The parent's forced skills are added to these.
    pub skills: SkillPolicy,
    /// 0 means the default.
    pub max_steps: u32,
    /// How many of this kind one parent run may create; 0 leaves it to the spawn policy.
    pub per_run: u32,
}

/// How a run ends on its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Finish {
    /// When the model answers without calling a tool. At depth 0 `report_to_inbox` is offered.
    #[default]
    Reply,
    /// When the model calls this tool with input that fits its schema; a model that stops
    /// without calling it is reminded twice.
    Tool(FinishTool),
}

/// The tool a pipeline node finishes with.
#[derive(Clone, Debug, PartialEq)]
pub struct FinishTool {
    pub name: String,
    pub description: String,
    /// JSON schema of the result; `required` and the top-level `type`s are checked.
    pub schema: Value,
}

impl FinishTool {
    /// `step_done` with this result schema.
    pub fn step_done(schema: Value) -> Self {
        Self {
            name: "step_done".into(),
            description: "Finish your step: hand over the result. Call it once, when the work is complete.".into(),
            schema,
        }
    }

    /// `step_done` taking only a Markdown `summary`.
    pub fn summary() -> Self {
        Self::step_done(json!({
            "type": "object",
            "properties": { "summary": { "type": "string", "description": "Markdown: what you did and how you checked it" } },
            "required": ["summary"]
        }))
    }
}

/// Sizes and patience of the loop. The defaults are what the app runs with; tests shrink them.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// Compact when the last request filled more than this share of the context window.
    pub compact_at: f64,
    /// Messages kept verbatim by a compaction (more when a tool call and its result would
    /// otherwise be split).
    pub keep_recent: usize,
    /// Bytes of one tool result the model reads; longer output is cut and kept as a blob.
    pub tool_output_max: usize,
    /// Retries of a model call after a retryable failure.
    pub retries: u32,
    /// Pause before the first retry; doubles each time up to `retry_max`.
    pub retry_base: Duration,
    pub retry_max: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            compact_at: 0.9,
            keep_recent: 8,
            tool_output_max: 48 * 1024,
            retries: 4,
            retry_base: Duration::from_secs(2),
            retry_max: Duration::from_secs(30),
        }
    }
}

/// Where the agent runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentEnv {
    pub session: SessionId,
    /// 0 for a chat or a pipeline node, 1 for its sub-agents, and so on.
    pub depth: u32,
    /// The folder file and shell tools work in; `None` leaves the agent with no such tools.
    pub workdir: Option<PathBuf>,
    /// The checkout permission rules are scoped to (a worktree's source repo). `None` scopes
    /// "Always allow" to the session.
    pub repo: Option<PathBuf>,
    /// Extra environment for processes tools start.
    pub env: BTreeMap<String, String>,
    /// Nothing may be written: tools that edit files or run commands are not offered.
    pub read_only: bool,
    /// `Plan` offers the same tools as `read_only` and asks for a plan.
    pub mode: ChatMode,
    /// The pipeline run and node this session belongs to, for the ledger.
    pub run: Option<RunId>,
    pub node: Option<NodeId>,
    /// The agent that created this one, when it is a sub-agent.
    pub created_by: Option<RunnableRef>,
}

/// What the agent may call. Everything is borrowed: the engine owns these for the whole app.
pub struct AgentDeps<'a> {
    pub llm: &'a dyn LlmClient,
    pub host: &'a dyn Host,
    pub gate: &'a dyn PermissionGate,
    pub mcp: Option<&'a dyn Mcp>,
    /// The skills installed when the run started.
    pub skills: &'a Catalog,
    pub clock: &'a dyn Clock,
    /// Tools offered next to the built-ins when there is a workdir; they pass the same grants,
    /// mode filter and permission gate. The access level applies to them by class: a tool
    /// that runs commands needs `ReadAndRun`, one that writes or reaches outside needs `Full`.
    pub tools: ToolRegistry,
}

/// A message from a person: text and images already in the blob store.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserInput {
    pub text: String,
    pub images: Vec<Attachment>,
}

impl UserInput {
    /// A message of text alone.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), images: Vec::new() }
    }
}

/// What starts a run.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A person's message.
    User(UserInput),
    /// A pipeline item or a sub-agent's task, as the text the agent reads.
    Item(String),
    /// Carry on after an interruption (a cancel, a crash, a limit) with what is stored.
    Continue,
    /// Summarise the older part of the conversation now; no conversation turn is made.
    Compact,
}

/// Why a run stopped.
#[derive(Clone, Debug, PartialEq)]
pub enum StopCause {
    /// The model ended its turn, or called the finish tool.
    Finished,
    /// The agent asked a person something with `ask_user`; the answer is the next
    /// [`Input::User`].
    Waiting { question: String },
    Cancelled,
    /// [`AgentSpec::step_limit`] model calls were made; [`Input::Continue`] carries on.
    StepLimit,
    /// The budget is spent.
    Budget,
    Failed { error: String },
}

/// What a run added up to. Cost and usage include the sub-agents it created; tool calls and
/// steps are its own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub cost_usd: f64,
    pub usage: Usage,
    pub tool_calls: u32,
    /// Model calls made.
    pub steps: u32,
    pub duration_ms: i64,
}

/// The result of [`crate::run`].
#[derive(Clone, Debug, PartialEq)]
pub struct AgentOutcome {
    pub stop: StopCause,
    /// The last text the model wrote in this run.
    pub text: String,
    /// The finish tool's input. `None` under [`Finish::Tool`] means the model never called it.
    pub payload: Option<Value>,
    pub totals: Totals,
    /// Every file the session has changed so far.
    pub files: Vec<FileChange>,
}

impl AgentOutcome {
    /// Whether the run stopped on its own: the model ended its turn or called the finish tool.
    /// Under [`Finish::Tool`] check `payload` as well: a model that never called the tool also
    /// ends here, without one.
    pub fn is_finished(&self) -> bool {
        self.stop == StopCause::Finished
    }
}
