//! The contract of this crate: what the executor asks of the outside world, and what it tells it.
//!
//! Signatures here are owned by the lead. Implementations live in the other modules.
//!
//! The executor is a state machine over a deployed graph and one run. It decides which node
//! instances may start, and what follows when one finishes or a person decides a review; it
//! does no work itself. The work of one instance (an agent turn loop, a shell command, a
//! router's question to a model) is a [`Job`] done by a [`NodeRunner`]. Tests drive the
//! machine with a scripted runner; the engine drives it with the real ones.
//!
//! How an item moves:
//!
//! - It enters at the source and follows `route` edges. Several edges from one port are
//!   parallel branches. A node starts when every `route` edge into it has either delivered the
//!   item or is dead (it leaves a port that did not fire, or a node that was skipped); it is
//!   skipped when all of them are dead.
//! - A router is one job that answers with the id of the rule that matched, or `else`.
//! - An agent or orchestrator with `variants = N` opens a variant scope: N instances, and
//!   every node after it runs once per variant until a review closes the scope. Each variant
//!   has its own worktree and branch, named by the runner.
//! - A review is not a job. The executor builds a [`model::Review`] from what the variants
//!   left behind and waits for a [`model::ReviewDecision`]. Ship continues along `approved`
//!   outside the scope with the picked variant's outputs; the other variants are discarded.
//!   Skip ends the run without anything after the review. Send back starts a new attempt of
//!   the review's `send_back_to` node with the note, up to the graph's `max_loops`.
//! - `context` and `spawn` edges never move the item. File and sub-agent nodes are handed to
//!   the job of the agent they are attached to.

use std::path::PathBuf;

use base::cancel::CancelToken;
use model::{
    FileChange, Issue, LogLevel, NodeDef, NodeId, Render, RunId, SessionId, SourceKind, StartFrom, TestResult, Usage,
};
use serde::{Deserialize, Serialize};

/// What flows through a run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub title: String,
    /// The issue body, the manual input, or the autopilot's prompt.
    pub text: String,
    /// The issue the run was started from; router conditions read its fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<Issue>,
    #[serde(default)]
    pub source: SourceKind,
}

/// One execution of one node: the node, one of its variants, one attempt. Matches
/// [`model::NodeInstance`]'s `node`, `variant` and `attempt`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InstanceKey {
    pub node: NodeId,
    /// 0 outside a variant scope, otherwise 1-based.
    pub variant: u32,
    /// Starts at 1; a sent-back round adds one.
    pub attempt: u32,
}

/// What an instance left behind for the nodes after it and for a review.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeOutput {
    /// One or two sentences: the agent's own summary, a router's reason, a command's result.
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// The folder the instance worked in, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Files changed against the base branch so far, with line counts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileChange>,
    /// Parsed counts of a `tests` command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestResult>,
    /// Pass or fail of a `tests` command when counts are not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renders: Vec<Render>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub usage: Usage,
}

/// The output of a node the item already passed on its way to a job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Upstream {
    pub key: InstanceKey,
    /// The node's name at the time of the run.
    pub name: String,
    pub output: NodeOutput,
}

/// How a sent-back attempt begins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Retry {
    /// What the person wrote, every round so far, oldest first.
    pub notes: Vec<String>,
    pub start: StartFrom,
    /// The branch and worktree to build on, when `start` keeps earlier work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_worktree: Option<PathBuf>,
    /// The session of the attempt being continued, so the agent keeps its conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_session: Option<SessionId>,
}

/// The work of one node instance, self-contained: a runner needs nothing but this and its own
/// services.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub run: RunId,
    pub key: InstanceKey,
    /// The node as deployed.
    pub node: NodeDef,
    /// File nodes attached by `context` edges and sub-agent templates attached by `spawn` edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attached: Vec<NodeDef>,
    pub item: Item,
    /// Outputs on the item's path to this node, in the order they finished. Inside a variant
    /// scope only this variant's instances, plus everything before the scope.
    #[serde(default)]
    pub upstream: Vec<Upstream>,
    /// Set on attempts after a "Send back".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// The person's answer when the instance stopped with [`JobResult::Ask`] and is continued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// The session of an interrupted instance that is being resumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session: Option<SessionId>,
    /// What this job may still spend before the run's or the node's budget is reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_left_usd: Option<f64>,
    /// A test run: throwaway worktrees, merges and pull requests are simulated.
    #[serde(default)]
    pub test: bool,
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum JobResult {
    /// The instance finished and the item leaves through `port`: `out` for most nodes, a rule
    /// id or `else` for a router.
    Done { port: String, output: NodeOutput },
    /// The agent needs a person before it can go on. The run waits; the answer comes back in
    /// a new job with the same key.
    Ask { question: String, output: NodeOutput },
    /// The job's budget ran out mid-work. The run waits for a person to raise it.
    OverBudget { output: NodeOutput },
    Failed { error: String, output: NodeOutput },
    /// The cancel token fired. The work done so far must be resumable.
    Cancelled { output: NodeOutput },
}

/// What a runner reports while a job is in progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "note", rename_all = "snake_case")]
pub enum JobNote {
    /// A line for the run's log.
    Log { level: LogLevel, text: String },
    /// The instance got its session, worktree or branch; known before the job ends so the UI
    /// can open them.
    Place {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    /// Money and tokens the job spent since its last note.
    Spent { cost_usd: f64, usage: Usage },
    /// The agent created a sub-agent, or one it created changed state.
    Spawned { id: String, name: String, session: Option<SessionId>, state: SpawnedState, summary: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnedState {
    Running,
    Done,
    Failed,
}

/// Where a runner sends [`JobNote`]s. Called from the job's own thread.
pub trait JobSink: Send + Sync {
    fn note(&self, note: JobNote);
}

/// Does the work of node instances. One runner serves every node type; it is called on a
/// worker thread per job and may block for as long as the work takes.
pub trait NodeRunner: Send + Sync {
    /// Runs the job to its end. Must return soon after `cancel` fires and must never panic:
    /// a failure is [`JobResult::Failed`].
    fn run(&self, job: &Job, sink: &dyn JobSink, cancel: &CancelToken) -> JobResult;

    /// Throws away what a variant left behind (its worktree and branch) after another variant
    /// shipped or a round started over. Best effort.
    fn discard(&self, run: &RunId, output: &NodeOutput);
}
