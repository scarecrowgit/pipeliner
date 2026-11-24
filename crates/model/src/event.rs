//! Events: what the engine tells the rest of the app. Each one names the single thing it is
//! about, so the hub can hand it to the one model entity that cares.

use serde::{Deserialize, Serialize};

use crate::{
    FileChange, InboxId, InboxItem, Issue, IssueEvent, IssueKey, PermissionRequest, RequestId, Review, Run, RunId,
    RunLogLine, RunWait, Runtime, Session, SessionId, StopReason, StoredMessage, TodoItem, ToolClass, ToolDisplay, Usage,
};

/// Everything a running agent reports, in order. The chat transcript and a pipeline node's log
/// are both built from this stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A model request started.
    TurnStarted,
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    /// A message is complete and stored: the user's, the assistant's, or tool results.
    MessageCommitted {
        message: StoredMessage,
    },
    /// The model began a tool call; its input is still streaming.
    ToolCallStarted {
        id: String,
        name: String,
    },
    /// The input is complete: this is what will run (or what a person is asked about).
    ToolCallReady {
        id: String,
        name: String,
        class: ToolClass,
        display: ToolDisplay,
    },
    ToolFinished {
        id: String,
        display: ToolDisplay,
        is_error: bool,
    },
    /// Sent after each model call.
    Usage {
        usage: Usage,
        cost_usd: f64,
        context_tokens: u64,
        context_window: u64,
    },
    TodoUpdated {
        items: Vec<TodoItem>,
    },
    FilesChanged {
        files: Vec<FileChange>,
    },
    SkillLoaded {
        name: String,
    },
    SubagentStarted {
        session: SessionId,
        name: String,
        task: String,
        depth: u32,
    },
    SubagentFinished {
        session: SessionId,
        summary: String,
        ok: bool,
        tool_calls: u32,
        secs: u64,
        cost_usd: f64,
    },
    CompactionStarted,
    Compacted {
        /// Messages replaced by the summary.
        summarised: u32,
        context_tokens: u64,
    },
    Retrying {
        attempt: u32,
        reason: String,
    },
    /// The agent opened or mentioned a pull request.
    PrDetected {
        url: String,
    },
    /// A line for the person that is not part of the conversation ("3 MCP servers started").
    Notice {
        text: String,
    },
    /// The agent stopped: finished its turn, was cancelled, or hit a limit.
    TurnFinished {
        stop: StopReason,
    },
    Failed {
        error: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// What the engine publishes. Whole values are sent for small things that change rarely
/// (a session's header, an issue, a run's summary); streams are sent as deltas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A session was created or its header changed (title, state, cost, pin, archive).
    SessionChanged { session: Session },
    SessionRemoved { session: SessionId },
    /// Something happened inside a running session.
    Agent { session: SessionId, event: AgentEvent },

    IssueChanged { issue: Issue },
    IssueRemoved { key: IssueKey },
    /// A new entry in an issue's activity feed.
    IssueActivity { key: IssueKey, event: IssueEvent },

    /// A run was created or its state or node instances changed. Logs are not included.
    RunChanged { run: Run },
    RunLog { run: RunId, line: RunLogLine },
    /// Why an open run waits for a person changed; `None` once it no longer waits.
    RunWaiting { run: RunId, waiting: Option<RunWait> },

    ReviewChanged { review: Review },

    InboxChanged { item: InboxItem },
    InboxRemoved { id: InboxId },

    PermissionRequested { request: PermissionRequest },
    /// Answered, or the session ended before anyone answered.
    PermissionResolved { id: RequestId },

    /// The library changed on disk or through the UI: re-read runnables, skills, pipelines.
    CatalogChanged,
    /// The ledger got new rows: re-read spend sums.
    SpendChanged,
    /// Settings or projects changed.
    SettingsChanged,
    AutopilotsChanged,
    RuntimeChanged { runtime: Runtime },

    /// Something to show as a toast.
    Notice { level: NoticeLevel, text: String },
}
