//! Tool calls waiting for a person, across every session: for Home and the sidebar.

use std::sync::Arc;

use gpui::Context;

use crate::api::{Api, PermissionRequest, RequestId, SessionFilter, SessionId, SessionState};
use crate::load::{LoadState, failed, query};

pub(crate) enum PermissionInput {
    Requested(Box<PermissionRequest>),
    Resolved(RequestId),
}

/// Pending permission requests, oldest first. Updated by `PermissionRequested` and
/// `PermissionResolved`.
pub struct PermissionsModel {
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    pending: Vec<Arc<PermissionRequest>>,
    buffer: Vec<PermissionInput>,
}

impl PermissionsModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self { api, state: LoadState::Loading, started: false, pending: Vec::new(), buffer: Vec::new() }
    }

    /// Whether the first load is done.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Every pending request, oldest first.
    pub fn all(&self) -> &[Arc<PermissionRequest>] {
        &self.pending
    }

    /// How many requests wait.
    pub fn count(&self) -> usize {
        self.pending.len()
    }

    /// One pending request by id.
    pub fn get(&self, id: &RequestId) -> Option<&Arc<PermissionRequest>> {
        self.pending.iter().find(|request| &request.id == id)
    }

    /// The pending requests of one session, oldest first.
    pub fn for_session<'a>(&'a self, session: &'a SessionId) -> impl Iterator<Item = &'a Arc<PermissionRequest>> + 'a {
        self.pending.iter().filter(move |request| &request.session == session)
    }

    /// The session a pending request belongs to.
    pub fn session_of(&self, id: &RequestId) -> Option<&SessionId> {
        self.get(id).map(|request| &request.session)
    }

    /// Reads the requests of every waiting session.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        query(
            cx,
            &self.api,
            |api| {
                let filter = SessionFilter { state: Some(SessionState::Waiting), ..SessionFilter::default() };
                let mut pending = Vec::new();
                for session in api.sessions(&filter)? {
                    if let Some(detail) = api.session_detail(&session.id)? {
                        pending.extend(detail.pending);
                    }
                }
                Ok(pending)
            },
            |this: &mut Self, result, cx| {
                match result {
                    Ok(pending) => {
                        this.pending = pending.into_iter().map(Arc::new).collect();
                        this.state = LoadState::Ready;
                        let buffered = std::mem::take(&mut this.buffer);
                        this.apply_inputs(buffered);
                    }
                    Err(error) => this.state = failed(&error),
                }
                cx.notify();
            },
        );
    }

    pub(crate) fn apply(&mut self, inputs: Vec<PermissionInput>, cx: &mut Context<Self>) {
        if !self.started {
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(inputs);
            return;
        }
        self.apply_inputs(inputs);
        cx.notify();
    }

    fn apply_inputs(&mut self, inputs: Vec<PermissionInput>) {
        for input in inputs {
            match input {
                PermissionInput::Requested(request) => {
                    if self.get(&request.id).is_none() {
                        self.pending.push(Arc::new(*request));
                    }
                }
                PermissionInput::Resolved(id) => self.pending.retain(|request| request.id != id),
            }
        }
        self.pending.sort_by_key(|request| request.at);
    }
}
