//! The runner's services over the engine: sessions in the store on the shared slots, the
//! Inbox and blobs, the tracker.

use std::sync::{Arc, Weak};

use agent::{AgentDeps, AgentEnv, AgentOutcome, AgentSink, AgentSpec, AllowAll, Input, StopCause};
use anyhow::{Result, anyhow};
use base::cancel::CancelToken;
use model::{
    Actor, AgentEvent, BlobId, Event, InboxId, InboxItem, InboxKind, IssueEventKind, IssueKey, IssueStatus, ModelRef,
    RepoHost, Runnable, RunnableRef, Session, SessionId, SessionOrigin, SessionState, WorkMode, Workdir,
};
use pipeline::runners::{AgentSessions, InboxPost, Records, SessionOpen, Tracker};
use tools::ToolRegistry;

use crate::engine::Inner;
use crate::host::StoreHost;

/// The engine, as long as it is open.
fn upgrade(inner: &Weak<Inner>) -> Result<Arc<Inner>> {
    inner.upgrade().ok_or_else(|| anyhow!("The app is shutting down"))
}

/// Pipeline sessions: stored like chats, run on the chats' slots, every tool call allowed.
pub(crate) struct Sessions(pub Weak<Inner>);

/// Forwards to the bus and to the runner.
struct Tee<'a> {
    inner: &'a Inner,
    on_event: &'a mut (dyn FnMut(&SessionId, &AgentEvent) + Send),
}

impl AgentSink for Tee<'_> {
    fn event(&mut self, session: &SessionId, event: AgentEvent) {
        (self.on_event)(session, &event);
        self.inner.publish(Event::Agent { session: session.clone(), event });
    }
}

fn cancelled() -> AgentOutcome {
    AgentOutcome { stop: StopCause::Cancelled, text: String::new(), payload: None, totals: Default::default(), files: Vec::new() }
}

impl AgentSessions for Sessions {
    fn open(&self, open: &SessionOpen) -> Result<SessionId> {
        let inner = upgrade(&self.0)?;
        let now = inner.now();
        let workdir = open.workdir.as_ref().map(|path| Workdir {
            path: path.clone(),
            repo: open.source.as_deref().unwrap_or(path).file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            branch: open.branch.clone(),
            host: RepoHost::Local,
            mode: if open.branch.is_some() { WorkMode::Worktree } else { WorkMode::InPlace },
            source: open.source.clone().filter(|_| open.branch.is_some()),
            runtime: "local".into(),
        });
        let session = Session {
            id: SessionId::new(base::id::new_id()),
            title: open.title.clone(),
            parent: None,
            depth: 0,
            runnable: open.runnable.clone(),
            mode: Default::default(),
            context_window: llm::pricing::context_window(&open.model),
            model: open.model.clone(),
            workdir,
            state: SessionState::Idle,
            origin: SessionOrigin::Run { run: open.run.clone(), node: open.node.clone(), variant: open.variant },
            issue: open.issue.clone(),
            pinned: false,
            archived: false,
            created: now,
            updated: now,
            message_count: 0,
            cost_usd: 0.0,
            usage: Default::default(),
            context_tokens: 0,
            forced_skills: Vec::new(),
            pr_url: None,
        };
        inner.store.upsert_session(&session)?;
        inner.publish(Event::SessionChanged { session: session.clone() });
        Ok(session.id)
    }

    fn spent(&self, session: &SessionId) -> f64 {
        self.0.upgrade().and_then(|inner| inner.spent_below(session).ok()).unwrap_or(0.0)
    }

    fn has_conversation(&self, session: &SessionId) -> bool {
        self.0.upgrade().and_then(|inner| inner.store.model_messages(session).ok()).is_some_and(|m| !m.is_empty())
    }

    fn run(
        &self,
        spec: &AgentSpec,
        env: &AgentEnv,
        input: Input,
        on_event: &mut (dyn FnMut(&SessionId, &AgentEvent) + Send),
        cancel: &CancelToken,
    ) -> AgentOutcome {
        let Some(inner) = self.0.upgrade() else { return cancelled() };
        let capacity = || inner.store.settings().map_or(1, |s| s.slots);
        let Some(slot) = inner.slots.acquire(capacity, cancel) else { return cancelled() };
        let mut spec = spec.clone();
        spec.limits = inner.limits.clone();
        let id = env.session.clone();
        inner.set_state(&id, SessionState::Running);
        let folder = env.workdir.clone();
        let catalog = inner.catalog_for(folder.as_deref());
        let host = StoreHost::new(&inner);
        let mcp = crate::chat::mcp_view(&inner, folder.as_deref());
        let deps = AgentDeps {
            llm: &*inner.llm,
            host: &host,
            gate: &AllowAll,
            mcp: Some(&mcp),
            skills: &catalog,
            clock: &*inner.clock,
            tools: ToolRegistry::new(),
        };
        let mut sink = Tee { inner: &inner, on_event };
        let outcome = agent::run(&spec, env, &deps, input, &mut sink, cancel);
        drop(slot);
        let state = match outcome.stop {
            StopCause::Finished => SessionState::Done,
            StopCause::Waiting { .. } => SessionState::Waiting,
            StopCause::Failed { .. } => SessionState::Failed,
            _ => SessionState::Idle,
        };
        let spent = inner.spent_below(&id).ok();
        let usage = inner.store.spend_for_session(&id).ok().map(|t| t.usage);
        let now = inner.now();
        let _ = inner.update_session(&id, |s| {
            s.state = state;
            s.updated = s.updated.max(now);
            if let Some(spent) = spent {
                s.cost_usd = spent;
            }
            if let Some(usage) = usage {
                s.usage = usage;
            }
        });
        outcome
    }

    fn runnable(&self, reference: &RunnableRef) -> Option<Runnable> {
        self.0.upgrade()?.catalog().get(reference).cloned()
    }

    fn default_model(&self) -> ModelRef {
        self.0.upgrade().map(|inner| inner.default_model()).unwrap_or_default()
    }
}

/// The Inbox and the blob store.
pub(crate) struct Inbox(pub Weak<Inner>);

impl Records for Inbox {
    fn post(&self, post: &InboxPost) -> Result<()> {
        let inner = upgrade(&self.0)?;
        let item = InboxItem {
            id: InboxId::new(base::id::new_id()),
            kind: InboxKind::Report,
            title: post.title.clone(),
            summary: post.summary.clone(),
            outcome: Some(post.outcome),
            source: post.source.clone(),
            issue: post.issue.clone(),
            session: post.session.clone(),
            run: Some(post.run.clone()),
            links: post.links.clone(),
            created: inner.now(),
            read: false,
            archived: false,
        };
        inner.store.upsert_inbox_item(&item)?;
        inner.publish(Event::InboxChanged { item });
        Ok(())
    }

    fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        upgrade(&self.0)?.store.put_blob(bytes)
    }
}

/// The issue tracker in the store.
pub(crate) struct Issues(pub Weak<Inner>);

impl Tracker for Issues {
    fn update_issue(&self, key: &IssueKey, status: Option<IssueStatus>, comment: &str) -> Result<()> {
        let inner = upgrade(&self.0)?;
        let mut issue = inner.store.issue(key)?.ok_or_else(|| anyhow!("There is no issue {key}"))?;
        if let Some(to) = status.filter(|to| *to != issue.status) {
            let from = issue.status;
            issue.status = to;
            issue.updated = inner.now();
            inner.save_issue(&issue)?;
            inner.activity(key, Actor::System, IssueEventKind::StatusChanged { from, to })?;
        }
        if !comment.trim().is_empty() {
            inner.activity(key, Actor::System, IssueEventKind::Note { text: comment.trim().to_string() })?;
        }
        Ok(())
    }

    fn pr_opened(&self, key: &IssueKey, url: &str) -> Result<()> {
        let inner = upgrade(&self.0)?;
        if let Some(mut issue) = inner.store.issue(key)? {
            issue.pr_url = Some(url.to_string());
            issue.updated = inner.now();
            inner.save_issue(&issue)?;
        }
        inner.activity(key, Actor::System, IssueEventKind::PrOpened { url: url.to_string() })
    }
}
