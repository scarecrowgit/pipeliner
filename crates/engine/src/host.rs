//! [`agent::Host`] over the store: what the agent loop keeps, and the queue of messages a
//! person sent while it was busy.

use agent::{ChildSession, Host, Report, UserInput};
use anyhow::Result;
use model::{
    BlobId, Event, FileChange, InboxId, InboxItem, InboxKind, RunnableRef, Session, SessionId, SessionOrigin,
    SessionState, StoredMessage, TodoItem, UsageEntry,
};

use crate::api::UserMessage;
use crate::engine::Inner;

/// The marker that keeps a session's compaction summary.
pub(crate) fn summary_marker(id: &SessionId) -> String {
    format!("summary:{id}")
}

/// The engine as the agent loop's host.
pub(crate) struct StoreHost<'a> {
    inner: &'a Inner,
}

impl<'a> StoreHost<'a> {
    pub fn new(inner: &'a Inner) -> Self {
        Self { inner }
    }
}

impl Host for StoreHost<'_> {
    fn messages(&self, session: &SessionId) -> Result<Vec<StoredMessage>> {
        self.inner.store.model_messages(session)
    }

    fn summary(&self, session: &SessionId) -> Result<Option<String>> {
        self.inner.store.marker(&summary_marker(session))
    }

    fn append(&self, session: &SessionId, message: StoredMessage) -> Result<StoredMessage> {
        self.inner.store.append_message(session, message)
    }

    fn compact(&self, session: &SessionId, up_to_seq: u64, summary: &str) -> Result<()> {
        self.inner.store.compact_messages(session, up_to_seq)?;
        self.inner.store.set_marker(&summary_marker(session), summary)
    }

    fn todos(&self, session: &SessionId) -> Result<Vec<TodoItem>> {
        self.inner.store.todos(session)
    }

    fn set_todos(&self, session: &SessionId, items: &[TodoItem]) -> Result<()> {
        self.inner.store.set_todos(session, items)
    }

    fn files(&self, session: &SessionId) -> Result<Vec<FileChange>> {
        self.inner.store.file_changes(session)
    }

    fn set_files(&self, session: &SessionId, files: &[FileChange]) -> Result<()> {
        self.inner.store.set_file_changes(session, files)
    }

    fn create_child(&self, parent: &SessionId, child: &ChildSession) -> Result<SessionId> {
        let parent_session = self.inner.session(parent)?;
        let id = SessionId::new(base::id::new_id());
        let now = self.inner.now();
        let session = Session {
            id: id.clone(),
            title: child.name.clone(),
            parent: Some(parent.clone()),
            depth: child.depth,
            runnable: RunnableRef::spawned(id.as_str()),
            mode: parent_session.mode,
            model: child.model.clone(),
            workdir: parent_session.workdir.clone(),
            state: SessionState::Running,
            origin: SessionOrigin::Agent { parent: parent.clone() },
            issue: parent_session.issue.clone(),
            pinned: false,
            archived: false,
            created: now,
            updated: now,
            message_count: 0,
            cost_usd: 0.0,
            usage: Default::default(),
            context_tokens: 0,
            context_window: llm::pricing::context_window(&child.model),
            forced_skills: Vec::new(),
            pr_url: None,
        };
        self.inner.store.upsert_session(&session)?;
        self.inner.publish(Event::SessionChanged { session });
        Ok(id)
    }

    fn record_usage(&self, entry: &UsageEntry) -> Result<()> {
        self.inner.store.record_usage(entry)?;
        self.inner.publish(Event::SpendChanged);
        Ok(())
    }

    fn spent(&self, session: &SessionId) -> Result<f64> {
        self.inner.spent_below(session)
    }

    fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        self.inner.store.put_blob(bytes)
    }

    fn read_blob(&self, id: &BlobId) -> std::io::Result<Vec<u8>> {
        self.inner.store.read_blob(id).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{e:#}")))
    }

    fn post_report(&self, session: &SessionId, report: &Report) -> Result<()> {
        let header = self.inner.session(session)?;
        let source = self
            .inner
            .catalog()
            .get(&header.runnable)
            .map_or_else(|| header.title.clone(), |r| format!("{} · chat", r.name));
        let item = InboxItem {
            id: InboxId::new(base::id::new_id()),
            kind: InboxKind::Report,
            title: report.title.clone(),
            summary: report.summary.clone(),
            outcome: Some(report.outcome),
            source,
            issue: header.issue.clone(),
            session: Some(session.clone()),
            run: None,
            links: report.links.clone(),
            created: self.inner.now(),
            read: false,
            archived: false,
        };
        self.inner.store.upsert_inbox_item(&item)?;
        self.inner.publish(Event::InboxChanged { item });
        Ok(())
    }

    fn take_queued(&self, session: &SessionId) -> Vec<UserInput> {
        self.inner.live.take_queued(session).into_iter().map(to_input).collect()
    }

    fn requeue(&self, session: &SessionId, inputs: Vec<UserInput>) {
        let messages = inputs.into_iter().map(|i| UserMessage { text: i.text, images: i.images }).collect();
        self.inner.live.requeue(session, messages);
    }
}

/// A person's message as the loop takes it.
pub(crate) fn to_input(message: UserMessage) -> UserInput {
    UserInput { text: message.text, images: message.images }
}

impl Inner {
    /// What a session and every session below it spent.
    pub fn spent_below(&self, session: &SessionId) -> Result<f64> {
        let mut total = 0.0;
        let mut stack = vec![session.clone()];
        while let Some(id) = stack.pop() {
            total += self.store.spend_for_session(&id)?.cost_usd;
            stack.extend(self.store.child_sessions(&id)?.into_iter().map(|child| child.id));
        }
        Ok(total)
    }
}
