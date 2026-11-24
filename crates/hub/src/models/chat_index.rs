//! The sidebar's chats: pinned and recent top-level sessions, the open session's sub-agents,
//! and the counts next to "Chats".

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use gpui::Context;

use crate::api::{Api, Session, SessionFilter, SessionId, SessionQuery};
use crate::load::{LoadState, failed, query};

/// How many recent sessions the index reads at start; older ones come in through events.
const LOAD_LIMIT: usize = 200;

pub(crate) enum IndexInput {
    Changed(Box<Session>),
    Removed(SessionId),
}

/// Sidebar data. Updated by `SessionChanged` and `SessionRemoved`.
pub struct ChatIndex {
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    /// Top-level, not archived.
    sessions: HashMap<SessionId, Arc<Session>>,
    pinned: Vec<Arc<Session>>,
    recent: Vec<Arc<Session>>,
    open: Option<SessionId>,
    children: Vec<Arc<Session>>,
    waiting: u64,
    total: u64,
    buffer: Vec<IndexInput>,
    count_generation: u64,
    children_generation: u64,
}

impl ChatIndex {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self {
            api,
            state: LoadState::Loading,
            started: false,
            sessions: HashMap::new(),
            pinned: Vec::new(),
            recent: Vec::new(),
            open: None,
            children: Vec::new(),
            waiting: 0,
            total: 0,
            buffer: Vec::new(),
            count_generation: 0,
            children_generation: 0,
        }
    }

    /// Whether the first load is done.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Pinned sessions, most recently active first.
    pub fn pinned(&self) -> &[Arc<Session>] {
        &self.pinned
    }

    /// Unpinned sessions, most recently active first.
    pub fn recent(&self) -> &[Arc<Session>] {
        &self.recent
    }

    /// The session the chat screen shows, if any.
    pub fn open(&self) -> Option<&SessionId> {
        self.open.as_ref()
    }

    /// Sub-agent sessions of the open session, oldest first.
    pub fn children(&self) -> &[Arc<Session>] {
        &self.children
    }

    /// Sessions waiting for a person ("1 waiting").
    pub fn waiting(&self) -> u64 {
        self.waiting
    }

    /// Every chat that is not archived ("View all 2,418").
    pub fn total(&self) -> u64 {
        self.total
    }

    /// A sidebar session or a sub-agent of the open one.
    pub fn session(&self, id: &SessionId) -> Option<&Arc<Session>> {
        self.sessions.get(id).or_else(|| self.children.iter().find(|s| &s.id == id))
    }

    /// Reads the list and the counts, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.started {
            self.reload(cx);
        }
    }

    /// Reads the list and the counts again.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.started = true;
        self.state = LoadState::Loading;
        query(
            cx,
            &self.api,
            |api| {
                let filter = SessionFilter { archived: Some(false), limit: Some(LOAD_LIMIT), ..SessionFilter::default() };
                Ok((api.sessions(&filter)?, counts(api)?))
            },
            |this: &mut Self, result, cx| {
                match result {
                    Ok((sessions, (total, waiting))) => {
                        this.sessions.clear();
                        for session in sessions {
                            this.upsert(session);
                        }
                        this.total = total;
                        this.waiting = waiting;
                        this.state = LoadState::Ready;
                        let buffered = std::mem::take(&mut this.buffer);
                        this.apply_inputs(buffered);
                        this.rebuild();
                    }
                    Err(error) => this.state = failed(&error),
                }
                cx.notify();
            },
        );
    }

    /// Tells the index which session is open, so it can list that session's sub-agents.
    pub fn set_open(&mut self, session: Option<SessionId>, cx: &mut Context<Self>) {
        if self.open == session {
            return;
        }
        self.open = session.clone();
        self.children.clear();
        self.children_generation += 1;
        cx.notify();
        let Some(id) = session else { return };
        let generation = self.children_generation;
        query(
            cx,
            &self.api,
            move |api| Ok(api.session_detail(&id)?.map(|detail| detail.children).unwrap_or_default()),
            move |this: &mut Self, result, cx| {
                if generation != this.children_generation {
                    return;
                }
                if let Ok(children) = result {
                    this.children = children.into_iter().map(Arc::new).collect();
                    cx.notify();
                }
            },
        );
    }

    pub(crate) fn apply(&mut self, inputs: Vec<IndexInput>, cx: &mut Context<Self>) {
        if !self.started {
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(inputs);
            return;
        }
        self.apply_inputs(inputs);
        self.rebuild();
        self.recount(cx);
        cx.notify();
    }

    fn apply_inputs(&mut self, inputs: Vec<IndexInput>) {
        for input in inputs {
            match input {
                IndexInput::Changed(session) => self.upsert(*session),
                IndexInput::Removed(id) => {
                    self.sessions.remove(&id);
                    self.children.retain(|child| child.id != id);
                }
            }
        }
    }

    fn upsert(&mut self, session: Session) {
        let session = Arc::new(session);
        if session.parent.is_some() && session.parent == self.open {
            match self.children.iter_mut().find(|child| child.id == session.id) {
                Some(child) => *child = session.clone(),
                None => self.children.push(session.clone()),
            }
        }
        if session.parent.is_none() && !session.archived {
            self.sessions.insert(session.id.clone(), session);
        } else {
            self.sessions.remove(&session.id);
        }
    }

    fn rebuild(&mut self) {
        let mut all: Vec<Arc<Session>> = self.sessions.values().cloned().collect();
        all.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
        let (pinned, recent) = all.into_iter().partition(|session| session.pinned);
        self.pinned = pinned;
        self.recent = recent;
    }

    /// Counts come from the store: the index holds only a window of sessions.
    fn recount(&mut self, cx: &mut Context<Self>) {
        self.count_generation += 1;
        let generation = self.count_generation;
        query(cx, &self.api, counts, move |this: &mut Self, result, cx| {
            if generation == this.count_generation
                && let Ok((total, waiting)) = result
                && (total, waiting) != (this.total, this.waiting)
            {
                this.total = total;
                this.waiting = waiting;
                cx.notify();
            }
        });
    }
}

/// (chats that are not archived, chats waiting for a person).
fn counts(api: &dyn Api) -> Result<(u64, u64)> {
    let result = api.search_sessions(&SessionQuery { limit: 0, ..SessionQuery::default() })?;
    Ok((result.facets.all, result.facets.waiting))
}
