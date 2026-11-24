//! [`MemoryHost`]: a [`Host`] that keeps everything in memory, for this crate's tests and for
//! the tests of crates that drive the loop.

use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::{Result, bail};
use model::{BlobId, FileChange, Message, SessionId, StoredMessage, TodoItem, UsageEntry};

use crate::host::{ChildSession, Host, Report};
use crate::sink::lock;
use crate::spec::UserInput;

#[derive(Default)]
struct SessionData {
    /// Every message, compacted ones included.
    messages: Vec<StoredMessage>,
    summary: Option<String>,
    todos: Vec<TodoItem>,
    files: Vec<FileChange>,
    queued: Vec<UserInput>,
    parent: Option<SessionId>,
    child: Option<ChildSession>,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<SessionId, SessionData>,
    blobs: Vec<Vec<u8>>,
    ledger: Vec<UsageEntry>,
    /// Spend added by a test, by session.
    extra_spend: Vec<(SessionId, f64)>,
    reports: Vec<(SessionId, Report)>,
    fail_appends: bool,
    fail_files: bool,
}

impl State {
    /// Whether `session` is `ancestor` or below it.
    fn is_under(&self, session: &SessionId, ancestor: &SessionId) -> bool {
        let mut at = Some(session);
        while let Some(id) = at {
            if id == ancestor {
                return true;
            }
            at = self.sessions.get(id).and_then(|s| s.parent.as_ref());
        }
        false
    }
}

/// Sessions, blobs, the ledger and the Inbox in memory. Sessions come into being when first
/// touched; sub-agent sessions get the ids `<parent>.1`, `<parent>.2`, in creation order.
#[derive(Default)]
pub struct MemoryHost {
    state: Mutex<State>,
}

impl MemoryHost {
    /// A host with no sessions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts messages into a session as if earlier runs had stored them.
    pub fn seed(&self, session: &SessionId, messages: impl IntoIterator<Item = Message>) {
        for message in messages {
            let stored = StoredMessage {
                seq: 0,
                at: 0,
                message,
                compacted: false,
                model: None,
                usage: Default::default(),
                cost_usd: 0.0,
            };
            // Appending to memory cannot fail unless a test asked for it.
            let _ = self.append(session, stored);
        }
    }

    /// Every stored message of a session, compacted ones included.
    pub fn stored(&self, session: &SessionId) -> Vec<StoredMessage> {
        lock(&self.state).sessions.get(session).map(|s| s.messages.clone()).unwrap_or_default()
    }

    /// The messages a model would still be sent, without their storage details.
    pub fn transcript(&self, session: &SessionId) -> Vec<Message> {
        self.stored(session).into_iter().filter(|m| !m.compacted).map(|m| m.message).collect()
    }

    /// Queues a message as a person does while the agent is busy.
    pub fn queue(&self, session: &SessionId, input: UserInput) {
        lock(&self.state).sessions.entry(session.clone()).or_default().queued.push(input);
    }

    /// Sub-agent sessions of `parent`, in creation order, with what they were created with.
    pub fn children(&self, parent: &SessionId) -> Vec<(SessionId, ChildSession)> {
        let state = lock(&self.state);
        let mut out: Vec<(SessionId, ChildSession)> = state
            .sessions
            .iter()
            .filter(|(_, s)| s.parent.as_ref() == Some(parent))
            .filter_map(|(id, s)| Some((id.clone(), s.child.clone()?)))
            .collect();
        out.sort_by_key(|(id, _)| id.as_str().rsplit('.').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0));
        out
    }

    /// The ledger, oldest first.
    pub fn ledger(&self) -> Vec<UsageEntry> {
        lock(&self.state).ledger.clone()
    }

    /// Counts `usd` as already spent by `session`, without a ledger row.
    pub fn add_spend(&self, session: &SessionId, usd: f64) {
        lock(&self.state).extra_spend.push((session.clone(), usd));
    }

    /// Reports posted to the Inbox, oldest first.
    pub fn reports(&self) -> Vec<(SessionId, Report)> {
        lock(&self.state).reports.clone()
    }

    /// The bytes of a stored blob.
    pub fn blob(&self, id: &BlobId) -> Option<Vec<u8>> {
        self.read_blob(id).ok()
    }

    /// Makes every later [`Host::append`] fail, as a broken store would.
    pub fn fail_appends(&self, fail: bool) {
        lock(&self.state).fail_appends = fail;
    }

    /// Makes every later [`Host::set_files`] fail, while messages can still be stored.
    pub fn fail_files(&self, fail: bool) {
        lock(&self.state).fail_files = fail;
    }
}

impl Host for MemoryHost {
    fn messages(&self, session: &SessionId) -> Result<Vec<StoredMessage>> {
        Ok(self.stored(session).into_iter().filter(|m| !m.compacted).collect())
    }

    fn summary(&self, session: &SessionId) -> Result<Option<String>> {
        Ok(lock(&self.state).sessions.get(session).and_then(|s| s.summary.clone()))
    }

    fn append(&self, session: &SessionId, message: StoredMessage) -> Result<StoredMessage> {
        let mut state = lock(&self.state);
        if state.fail_appends {
            bail!("the store is not writable");
        }
        let data = state.sessions.entry(session.clone()).or_default();
        let seq = data.messages.last().map_or(1, |m| m.seq + 1);
        let stored = StoredMessage { seq, ..message };
        data.messages.push(stored.clone());
        Ok(stored)
    }

    fn compact(&self, session: &SessionId, up_to_seq: u64, summary: &str) -> Result<()> {
        let mut state = lock(&self.state);
        let data = state.sessions.entry(session.clone()).or_default();
        for message in data.messages.iter_mut().filter(|m| m.seq <= up_to_seq) {
            message.compacted = true;
        }
        data.summary = Some(summary.to_string());
        Ok(())
    }

    fn todos(&self, session: &SessionId) -> Result<Vec<TodoItem>> {
        Ok(lock(&self.state).sessions.get(session).map(|s| s.todos.clone()).unwrap_or_default())
    }

    fn set_todos(&self, session: &SessionId, items: &[TodoItem]) -> Result<()> {
        lock(&self.state).sessions.entry(session.clone()).or_default().todos = items.to_vec();
        Ok(())
    }

    fn files(&self, session: &SessionId) -> Result<Vec<FileChange>> {
        Ok(lock(&self.state).sessions.get(session).map(|s| s.files.clone()).unwrap_or_default())
    }

    fn set_files(&self, session: &SessionId, files: &[FileChange]) -> Result<()> {
        let mut state = lock(&self.state);
        if state.fail_files {
            bail!("the file list is not writable");
        }
        state.sessions.entry(session.clone()).or_default().files = files.to_vec();
        Ok(())
    }

    fn create_child(&self, parent: &SessionId, child: &ChildSession) -> Result<SessionId> {
        let mut state = lock(&self.state);
        let n = state.sessions.values().filter(|s| s.parent.as_ref() == Some(parent)).count() + 1;
        let id = SessionId::new(format!("{parent}.{n}"));
        let data = SessionData { parent: Some(parent.clone()), child: Some(child.clone()), ..Default::default() };
        state.sessions.insert(id.clone(), data);
        Ok(id)
    }

    fn record_usage(&self, entry: &UsageEntry) -> Result<()> {
        lock(&self.state).ledger.push(entry.clone());
        Ok(())
    }

    fn spent(&self, session: &SessionId) -> Result<f64> {
        let state = lock(&self.state);
        let ledger: f64 = state
            .ledger
            .iter()
            .filter(|e| e.session.as_ref().is_some_and(|s| state.is_under(s, session)))
            .map(|e| e.cost_usd)
            .sum();
        let extra: f64 = state.extra_spend.iter().filter(|(s, _)| state.is_under(s, session)).map(|(_, usd)| usd).sum();
        Ok(ledger + extra)
    }

    fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        let mut state = lock(&self.state);
        state.blobs.push(bytes.to_vec());
        Ok(BlobId::new(format!("blob-{}", state.blobs.len())))
    }

    fn read_blob(&self, id: &BlobId) -> std::io::Result<Vec<u8>> {
        let state = lock(&self.state);
        id.as_str()
            .strip_prefix("blob-")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| state.blobs.get(n.checked_sub(1)?))
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("no blob {id}")))
    }

    fn post_report(&self, session: &SessionId, report: &Report) -> Result<()> {
        lock(&self.state).reports.push((session.clone(), report.clone()));
        Ok(())
    }

    fn take_queued(&self, session: &SessionId) -> Vec<UserInput> {
        lock(&self.state).sessions.get_mut(session).map(|s| std::mem::take(&mut s.queued)).unwrap_or_default()
    }
}
