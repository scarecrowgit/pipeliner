//! [`Host`]: everything the loop keeps outside itself. The engine implements it over the
//! store; [`crate::MemoryHost`] implements it in memory for tests.

use anyhow::Result;
use model::{
    BlobId, FileChange, Link, ModelRef, Outcome, RunnableRef, SessionId, StoredMessage, TodoItem, UsageEntry,
};

use crate::spec::UserInput;

/// A sub-agent session to create.
#[derive(Clone, Debug, PartialEq)]
pub struct ChildSession {
    /// Display name: the template's, or the label the parent gave.
    pub name: String,
    /// What it was asked to do; also its first message.
    pub task: String,
    pub model: ModelRef,
    pub depth: u32,
    /// The agent that creates it.
    pub created_by: RunnableRef,
}

/// What an agent posts to the Inbox with `report_to_inbox`.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub title: String,
    /// Markdown.
    pub summary: String,
    pub outcome: Outcome,
    pub links: Vec<Link>,
}

/// The session state and the services behind one agent run. Calls may come from several
/// threads at once (sub-agents run in parallel), each for its own session. One session has
/// one run at a time: the loop mirrors the stored messages in memory and does not expect
/// another writer while it runs.
pub trait Host: Send + Sync {
    /// The messages still sent to the model: every one not replaced by a summary, in order.
    fn messages(&self, session: &SessionId) -> Result<Vec<StoredMessage>>;

    /// The summary stored by the latest [`Host::compact`], if there was one.
    fn summary(&self, session: &SessionId) -> Result<Option<String>>;

    /// Appends a message (its `seq` is ignored) and returns it as stored.
    fn append(&self, session: &SessionId, message: StoredMessage) -> Result<StoredMessage>;

    /// Marks every message up to and including `up_to_seq` as replaced by `summary`, which
    /// replaces any earlier summary.
    fn compact(&self, session: &SessionId, up_to_seq: u64, summary: &str) -> Result<()>;

    /// The session's task list, as `todo_write` last set it.
    fn todos(&self, session: &SessionId) -> Result<Vec<TodoItem>>;
    /// Replaces the session's task list.
    fn set_todos(&self, session: &SessionId, items: &[TodoItem]) -> Result<()>;

    /// Files the session changed, with line counts summed over its edits.
    fn files(&self, session: &SessionId) -> Result<Vec<FileChange>>;
    /// Replaces the list of files the session changed.
    fn set_files(&self, session: &SessionId, files: &[FileChange]) -> Result<()>;

    /// Creates the session of a sub-agent of `parent` and returns its id.
    fn create_child(&self, parent: &SessionId, child: &ChildSession) -> Result<SessionId>;

    /// Adds one model call to the ledger.
    fn record_usage(&self, entry: &UsageEntry) -> Result<()>;

    /// What the session and every session below it have spent, in USD.
    fn spent(&self, session: &SessionId) -> Result<f64>;

    /// Stores bytes (a long tool output) and returns their id.
    fn put_blob(&self, bytes: &[u8]) -> Result<BlobId>;

    /// Reads a blob; image attachments are sent to the model through this.
    fn read_blob(&self, id: &BlobId) -> std::io::Result<Vec<u8>>;

    /// Posts an agent's report to the Inbox.
    fn post_report(&self, session: &SessionId, report: &Report) -> Result<()>;

    /// Takes the messages a person sent while the agent was busy, oldest first. The loop
    /// calls this before every model call and when the model ends its turn.
    fn take_queued(&self, session: &SessionId) -> Vec<UserInput>;
}

/// The host as the model client's source of image bytes.
pub(crate) struct HostBlobs<'a>(pub &'a dyn Host);

impl llm::BlobSource for HostBlobs<'_> {
    fn read(&self, id: &BlobId) -> std::io::Result<Vec<u8>> {
        self.0.read_blob(id)
    }
}
