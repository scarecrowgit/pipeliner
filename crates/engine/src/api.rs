//! The contract between the domain and everything that drives it: the GUI (through `hub`),
//! the CLI, tests. Owned by the lead.
//!
//! Three kinds of methods:
//! - **Queries** read state and may block on disk or, where noted, on the network. The GUI
//!   calls them off the UI thread.
//! - **Commands** change state. They return as soon as the change is recorded or the work is
//!   handed to a worker; results arrive as [`Event`]s.
//! - [`Api::subscribe`] is the event stream.
//!
//! Every method is safe to call from any thread.

use std::path::{Path, PathBuf};

use anyhow::Result;

pub use catalog::{LoadProblem, Place, Template};
pub use llm::ProviderInfo;
pub use model::*;
pub use store::{
    FacetCounts, IndexStats, MessageSnippet, ParentFilter, SearchResult, SessionFilter, SessionHit, SessionQuery,
    StartedBy,
};
pub use vcs::{FolderInfo, RepoFacts};

/// Everything the app can do. Implemented by the engine, and by a fixture backend for previews.
pub trait Api: ChatApi + WorkdirApi + IssueApi + RunApi + InboxApi + CatalogApi + OpsApi + Send + Sync {
    /// A new receiver of every event from now on. Each subscriber gets every event.
    fn subscribe(&self) -> async_channel::Receiver<Event>;
}

// ---------------------------------------------------------------- chat

/// Where a new session works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkdirChoice {
    /// The folder to work in; for `WorkMode::Worktree` the checkout to cut the worktree from.
    pub path: PathBuf,
    pub mode: WorkMode,
    /// Worktree mode: the branch to start from.
    pub base: Option<String>,
    /// Worktree mode: the new branch name.
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewSession {
    /// Who answers; `None` for the default chat agent.
    pub runnable: Option<RunnableRef>,
    pub mode: ChatMode,
    /// `None` for the runnable's own model, or the configured default.
    pub model: Option<ModelRef>,
    /// `None` starts a session with no folder: the agent can only talk.
    pub workdir: Option<WorkdirChoice>,
    pub forced_skills: Vec<String>,
    pub issue: Option<IssueKey>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserMessage {
    pub text: String,
    pub images: Vec<Attachment>,
}

/// A skill as the session's context panel lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInContext {
    pub name: String,
    /// In every request from the start, as opposed to loaded when relevant.
    pub forced: bool,
    /// Its full text is in the context right now.
    pub loaded: bool,
    pub tokens: u64,
}

/// Everything the chat screen shows around the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionDetail {
    pub session: Session,
    pub todos: Vec<TodoItem>,
    pub files: Vec<FileChange>,
    /// Sub-agent sessions, oldest first.
    pub children: Vec<Session>,
    pub skills: Vec<SkillInContext>,
    pub mcp: Vec<McpServerInfo>,
    /// Git facts of the workdir, when it is a repo.
    pub repo: Option<RepoFacts>,
    /// Tool calls waiting for an answer in this session.
    pub pending: Vec<PermissionRequest>,
    /// Messages sent while the agent was busy, delivered at its next turn.
    pub queued: Vec<UserMessage>,
}

/// A provider and the models it offers right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderModels {
    pub provider: ProviderInfo,
    pub models: Vec<String>,
    /// Why the live list could not be fetched; `models` then holds the configured ones.
    pub error: Option<String>,
}

/// A `/` command the composer offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    /// Comes from the project's `.harness/commands`, as opposed to built in.
    pub from_project: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    /// One JSON object per message, with tool calls.
    Jsonl,
}

/// What a transcript export includes. Secrets and tokens are always redacted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub subagents: bool,
    pub tool_output: bool,
    pub images: bool,
    /// Keep absolute local paths instead of showing them relative to the repo.
    pub local_paths: bool,
}

pub trait ChatApi {
    fn sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>>;
    fn session_detail(&self, id: &SessionId) -> Result<Option<SessionDetail>>;
    /// Stored messages with `seq` in `from..to`, oldest first, compacted ones included.
    fn messages(&self, id: &SessionId, from: u64, to: u64) -> Result<Vec<StoredMessage>>;
    fn search_sessions(&self, query: &SessionQuery) -> Result<SearchResult>;

    /// Creates the session and, for worktree mode, the worktree. Nothing runs until a message
    /// is sent.
    fn new_session(&self, new: NewSession) -> Result<Session>;
    /// Sends a message. A running session queues it for its next turn.
    fn send(&self, id: &SessionId, message: UserMessage) -> Result<()>;
    /// Stops the running turn; the session becomes idle with what it has so far.
    fn interrupt(&self, id: &SessionId);
    /// Runs the last failed or interrupted turn again.
    fn retry(&self, id: &SessionId) -> Result<()>;
    /// Summarises older turns now instead of waiting for the threshold.
    fn compact(&self, id: &SessionId) -> Result<()>;
    fn answer_permission(&self, request: &RequestId, decision: PermissionDecision) -> Result<()>;

    /// Switches who answers in this thread from the next message on; the workdir stays.
    fn set_session_runnable(&self, id: &SessionId, runnable: &RunnableRef, mode: ChatMode) -> Result<()>;
    fn set_session_model(&self, id: &SessionId, model: &ModelRef) -> Result<()>;
    /// The next message runs in the new folder; uncommitted changes stay where they are.
    fn set_session_workdir(&self, id: &SessionId, workdir: WorkdirChoice) -> Result<Session>;
    fn set_forced_skills(&self, id: &SessionId, skills: Vec<String>) -> Result<()>;
    fn rename_session(&self, id: &SessionId, title: &str) -> Result<()>;
    fn pin_session(&self, id: &SessionId, pinned: bool) -> Result<()>;
    fn archive_session(&self, id: &SessionId, archived: bool) -> Result<()>;
    fn delete_session(&self, id: &SessionId) -> Result<()>;
    /// A new session with a copy of the conversation up to and including `up_to_seq`
    /// (`None` for all of it).
    fn fork_session(&self, id: &SessionId, up_to_seq: Option<u64>) -> Result<Session>;
    /// Writes the transcript to a file and returns its path.
    fn export_session(&self, id: &SessionId, format: ExportFormat, options: ExportOptions, to: &Path) -> Result<PathBuf>;

    /// Stores image bytes and returns the attachment to put in a message or an issue.
    fn put_image(&self, name: &str, media_type: &str, bytes: &[u8]) -> Result<Attachment>;
    /// Where a blob's bytes are on disk, for showing an image.
    fn blob_path(&self, id: &BlobId) -> PathBuf;

    /// Configured providers with their live model lists. Reaches the network.
    fn models(&self) -> Vec<ProviderModels>;
    /// Files of the session's workdir matching what was typed after `@`.
    fn mention_files(&self, id: &SessionId, prefix: &str, limit: usize) -> Vec<String>;
    fn slash_commands(&self, id: &SessionId) -> Vec<SlashCommand>;
    fn reconnect_mcp(&self, id: &SessionId, server: &str) -> Result<()>;
}

// ---------------------------------------------------------------- workdirs

/// A folder offered on the "new chat" page and in the workdir popover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkdirEntry {
    pub path: PathBuf,
    pub repo: String,
    pub branch: Option<String>,
    pub host: RepoHost,
    /// A worktree the app created, as opposed to a checkout.
    pub is_worktree: bool,
    /// "clean", "2 uncommitted · 1 ahead of main", "no remote".
    pub status: String,
    /// Who works there now: an agent's name, "this chat", "your chat".
    pub owner: Option<String>,
    /// Sessions that used it.
    pub sessions: u32,
}

pub trait WorkdirApi {
    /// Folders used recently, most recent first.
    fn recent_workdirs(&self) -> Vec<WorkdirEntry>;
    /// Worktrees of a repo, with who works in each.
    fn worktrees(&self, repo: &Path) -> Vec<WorkdirEntry>;
    fn repo_facts(&self, path: &Path) -> Option<RepoFacts>;
    fn branches(&self, repo: &Path) -> Vec<String>;
    /// What "Found in this folder" shows.
    fn folder_info(&self, path: &Path) -> FolderInfo;
    /// A free branch name for a new chat in this repo: `chat/<slug of the hint>`.
    fn suggest_branch(&self, repo: &Path, hint: &str) -> String;
    /// Clones into `dest` on a worker. Progress and the result arrive as notices.
    fn clone_repo(&self, url: &str, dest: &Path) -> Result<()>;
}

// ---------------------------------------------------------------- issues

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewIssue {
    pub project: ProjectId,
    pub title: String,
    pub body: String,
    pub status: IssueStatus,
    pub priority: Priority,
    pub kind: IssueType,
    pub owner: String,
    pub repo: Option<String>,
    pub runner: Option<RunnableRef>,
    pub triggers: StartTriggers,
    pub images: Vec<Attachment>,
}

/// Fields to change on an issue; `None` leaves a field as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IssuePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<IssueStatus>,
    pub priority: Option<Priority>,
    pub kind: Option<IssueType>,
    pub owner: Option<String>,
    /// `Some(None)` clears the repo.
    pub repo: Option<Option<String>>,
    pub triggers: Option<StartTriggers>,
}

pub trait IssueApi {
    fn issues(&self, project: &ProjectId) -> Result<Vec<Issue>>;
    fn issue(&self, key: &IssueKey) -> Result<Option<Issue>>;
    fn issue_events(&self, key: &IssueKey) -> Result<Vec<IssueEvent>>;

    /// Creates the issue and starts its runner when the triggers say so.
    fn create_issue(&self, new: NewIssue) -> Result<Issue>;
    /// Applies the patch; a status change may start or question a run, as the triggers say.
    fn update_issue(&self, key: &IssueKey, patch: IssuePatch) -> Result<Issue>;
    fn delete_issue(&self, key: &IssueKey) -> Result<()>;
    /// Posts a comment and wakes whoever the triggers and mentions say.
    fn comment(&self, key: &IssueKey, text: &str, images: Vec<Attachment>) -> Result<()>;
    /// Attaches a runner (and starts it when `on_assign` is set), or detaches with `None`.
    fn attach_runner(&self, key: &IssueKey, runner: Option<RunnableRef>) -> Result<Issue>;
    /// Starts the attached runner on the issue now, whatever the triggers say.
    fn start_issue(&self, key: &IssueKey) -> Result<()>;
    /// Tries again to wake a runnable whose wake-up failed.
    fn retry_wake(&self, key: &IssueKey, target: &RunnableRef) -> Result<()>;

    fn autopilots(&self, project: &ProjectId) -> Result<Vec<Autopilot>>;
    fn save_autopilot(&self, autopilot: Autopilot) -> Result<()>;
    fn delete_autopilot(&self, id: &AutopilotId) -> Result<()>;
}

// ---------------------------------------------------------------- runs

/// What a pipeline run starts with when it is not started from an issue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunInput {
    pub title: String,
    pub text: String,
}

pub trait RunApi {
    fn run(&self, id: &RunId) -> Result<Option<Run>>;
    /// The latest run of every issue that has one, plus runs without an issue that are open.
    fn latest_runs(&self) -> Result<Vec<Run>>;
    fn runs_for_issue(&self, key: &IssueKey) -> Result<Vec<Run>>;
    /// Log lines with `seq` greater than `after_seq`, oldest first.
    fn run_log(&self, id: &RunId, after_seq: u64, limit: usize) -> Result<Vec<RunLogLine>>;
    fn run_log_tail(&self, id: &RunId, count: usize) -> Result<Vec<RunLogLine>>;
    fn run_stats(&self, id: &RunId) -> Result<RunStats>;

    /// Starts the live version of a pipeline on a manual item.
    fn start_run(&self, pipeline: &PipelineId, input: RunInput) -> Result<Run>;
    /// Runs the draft in throwaway worktrees: real agents, simulated merges and pull requests.
    fn test_run(&self, pipeline: &PipelineId, input: RunInput) -> Result<Run>;
    fn cancel_run(&self, id: &RunId) -> Result<()>;
    /// Continues an interrupted or failed run from where it stopped.
    fn resume_run(&self, id: &RunId) -> Result<()>;
    /// Answers the question an agent of the run waits on: the instance of `node` when given,
    /// else the only one that asks.
    fn answer_run(&self, id: &RunId, node: Option<&NodeId>, text: &str) -> Result<()>;
    /// Allows the run to spend `usd` more, and continues what stopped at its budget.
    fn raise_run_budget(&self, id: &RunId, usd: f64) -> Result<()>;
}

// ---------------------------------------------------------------- reviews and inbox

/// What a sent-back round is expected to take, from the variants' own history.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    pub secs: u64,
    pub cost_usd: f64,
}

pub trait InboxApi {
    fn inbox(&self, archived: bool) -> Result<Vec<InboxItem>>;
    fn set_inbox_read(&self, id: &InboxId, read: bool) -> Result<()>;
    fn archive_inbox(&self, id: &InboxId, archived: bool) -> Result<()>;

    fn open_reviews(&self) -> Result<Vec<Review>>;
    fn review(&self, id: &ReviewId) -> Result<Option<Review>>;
    /// The diff of one file of one variant against its base.
    fn variant_diff(&self, id: &ReviewId, variant: u32, path: &str) -> Result<FileDiff>;
    fn send_back_estimate(&self, id: &ReviewId, start: StartFrom, variants: u32) -> Result<Estimate>;
    /// Ships, skips or sends back; the run continues on a worker.
    fn decide_review(&self, id: &ReviewId, decision: ReviewDecision) -> Result<()>;

    /// The live status of a pull request. Reaches the network.
    fn pull_request(&self, url: &str) -> Result<PullRequest>;
}

// ---------------------------------------------------------------- catalog

/// What deleting a runnable would break, for the confirmation dialog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsedBy {
    /// Issues it is attached to; the ones with an open run first.
    pub issues: Vec<IssueKey>,
    pub pipelines: Vec<PipelineId>,
    /// Agents it created that are still alive.
    pub created: Vec<RunnableRef>,
}

pub trait CatalogApi {
    fn runnables(&self) -> Vec<Runnable>;
    /// State and activity numbers for every runnable.
    fn runnable_activity(&self) -> Result<Vec<(RunnableRef, RunnableActivity)>>;
    fn skills(&self) -> Vec<Skill>;
    fn catalog_problems(&self) -> Vec<LoadProblem>;
    fn used_by(&self, reference: &RunnableRef) -> Result<UsedBy>;

    /// Writes an agent or orchestrator to the library (or the project) and reloads.
    fn save_runnable(&self, place: &Place, runnable: Runnable) -> Result<()>;
    fn duplicate_runnable(&self, reference: &RunnableRef) -> Result<RunnableRef>;
    fn rename_runnable(&self, reference: &RunnableRef, name: &str) -> Result<RunnableRef>;
    fn set_runnable_paused(&self, reference: &RunnableRef, paused: bool) -> Result<()>;
    fn set_runnable_archived(&self, reference: &RunnableRef, archived: bool) -> Result<()>;
    fn delete_runnable(&self, reference: &RunnableRef) -> Result<()>;
    fn templates(&self) -> Vec<Template>;
    fn create_from_template(&self, template: &str) -> Result<RunnableRef>;
    fn import_runnable(&self, file: &Path) -> Result<RunnableRef>;
    fn export_runnable(&self, reference: &RunnableRef, to: &Path) -> Result<PathBuf>;

    /// The draft of a pipeline: the file on disk.
    fn pipeline(&self, id: &PipelineId) -> Option<GraphDef>;
    fn save_pipeline(&self, place: &Place, graph: GraphDef) -> Result<()>;
    /// What the Problems tab shows for a graph, saved or not.
    fn pipeline_problems(&self, graph: &GraphDef) -> Vec<Problem>;
    fn pipeline_versions(&self, id: &PipelineId) -> Result<Vec<PipelineVersion>>;
    /// The draft against the live version.
    fn pipeline_changes(&self, id: &PipelineId) -> Result<Vec<GraphChange>>;
    /// Deploys the draft as the next version. Running runs stay on theirs.
    fn deploy_pipeline(&self, id: &PipelineId, note: &str) -> Result<PipelineVersion>;
}

// ---------------------------------------------------------------- home, spend, settings

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Period {
    #[default]
    Today,
    Days7,
    Days30,
    Days90,
}

/// The stat strip of Home.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeStats {
    pub working_now: u32,
    pub slots: u32,
    pub waiting: u32,
    /// When the oldest waiting item started waiting.
    pub oldest_waiting: Option<Millis>,
    pub runs: u32,
    /// Share of the period's runs that finished, 0 to 100.
    pub finished_percent: u32,
    pub spend_usd: f64,
    /// The limit scaled to the period; `None` when no limit is set.
    pub limit_usd: Option<f64>,
    /// Pull requests and specs produced.
    pub shipped: u32,
}

/// Where a row of Home leads when clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Session(SessionId),
    Issue(IssueKey),
    Run(RunId),
    Review(ReviewId),
    Inbox(InboxId),
}

/// What a person is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedKind {
    /// Allow or reject a tool call.
    Permission,
    /// Pick a variant or approve a render.
    Review,
    /// Answer an agent's question.
    Question,
    /// Look at a failed run.
    Failure,
}

/// A row of "Needs you".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub kind: NeedKind,
    pub title: String,
    /// "Build in chat · fix/PAY-1423".
    pub source: String,
    pub since: Millis,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkingState {
    /// `percent` is `None` when progress cannot be told (a chat turn).
    Running { percent: Option<u32> },
    /// With the question in a few words: "asks to run bun test".
    Waiting { text: String },
}

/// A row of "Working now".
#[derive(Clone, Debug, PartialEq)]
pub struct Working {
    pub name: String,
    /// "Orchestrator", "Pipeline · Reproducer", "Agent · chat", "created by Payments lead".
    pub kind: String,
    pub runnable_kind: RunnableKind,
    /// Created by another agent; drawn in violet.
    pub spawned: bool,
    pub task: String,
    /// Issue key or branch.
    pub key: String,
    pub runtime: String,
    pub since: Millis,
    pub cost_usd: f64,
    pub state: WorkingState,
    pub target: Target,
}

pub trait OpsApi {
    fn home_stats(&self, period: Period) -> Result<HomeStats>;
    fn needs_you(&self) -> Result<Vec<Need>>;
    fn working_now(&self) -> Result<Vec<Working>>;

    fn spend_today(&self) -> Result<SpendToday>;
    /// Bars of the spend chart: days for up to 30 days, weeks for 90.
    fn spend_buckets(&self, period: Period) -> Result<Vec<SpendBucket>>;
    /// The biggest spenders of the period, then one "Others" row.
    fn spend_by_runnable(&self, period: Period, top: usize) -> Result<Vec<SpendByRunnable>>;

    /// The local machine, plus the runtimes that are drawn but not available yet.
    fn runtimes(&self) -> Vec<Runtime>;

    fn settings(&self) -> Result<Settings>;
    fn set_settings(&self, settings: Settings) -> Result<()>;
    fn projects(&self) -> Result<Vec<Project>>;
    fn save_project(&self, project: Project) -> Result<()>;
    fn delete_project(&self, id: &ProjectId) -> Result<()>;

    fn permission_rules(&self) -> Result<Vec<PermissionRule>>;
    fn remove_permission_rule(&self, rule: &PermissionRule) -> Result<()>;

    /// Stops workers and marks what was running as interrupted. Call once, on quit.
    fn shutdown(&self);
}
