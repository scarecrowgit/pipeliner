//! Permissions: the rules kept in the store, and questions to a person that wait for
//! [`crate::api::ChatApi::answer_permission`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use agent::RuleStore;
use anyhow::{Result, bail};
use base::cancel::CancelToken;
use model::{Event, PermissionDecision, PermissionRequest, PermissionRule, RequestId, SessionId, SessionState};

use crate::bus::lock;
use crate::engine::Inner;

/// How often a waiting question looks at its cancel token.
const ASK_POLL: Duration = Duration::from_millis(20);

struct Pending {
    request: PermissionRequest,
    reply: Sender<PermissionDecision>,
}

/// Questions waiting for a person.
#[derive(Default)]
pub(crate) struct Asks {
    pending: Mutex<HashMap<RequestId, Pending>>,
}

impl Asks {
    /// Every open question, oldest first.
    pub fn all(&self) -> Vec<PermissionRequest> {
        let mut out: Vec<PermissionRequest> = lock(&self.pending).values().map(|p| p.request.clone()).collect();
        out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        out
    }

    /// The open questions of one session.
    pub fn of(&self, session: &SessionId) -> Vec<PermissionRequest> {
        self.all().into_iter().filter(|r| &r.session == session).collect()
    }

    /// Delivers a person's answer. Fails when nobody waits for it any more.
    pub fn answer(&self, id: &RequestId, decision: PermissionDecision) -> Result<()> {
        let Some(pending) = lock(&self.pending).remove(id) else {
            bail!("The request was already answered or its turn stopped");
        };
        // The asking thread may have just given up after a cancel; then nobody acts on it.
        let _ = pending.reply.send(decision);
        Ok(())
    }
}

impl Inner {
    /// Asks a person and blocks until they answer, or `None` soon after `cancel` fires.
    pub fn ask_person(&self, request: PermissionRequest, cancel: &CancelToken) -> Option<PermissionDecision> {
        if cancel.is_cancelled() {
            return None;
        }
        let (id, session) = (request.id.clone(), request.session.clone());
        let (tx, rx) = mpsc::channel();
        lock(&self.asks.pending).insert(id.clone(), Pending { request: request.clone(), reply: tx });
        self.set_state(&session, SessionState::Waiting);
        self.publish(Event::PermissionRequested { request });
        let answer = loop {
            if cancel.is_cancelled() {
                break None;
            }
            match rx.recv_timeout(ASK_POLL) {
                Ok(decision) => break Some(decision),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break None,
            }
        };
        lock(&self.asks.pending).remove(&id);
        self.publish(Event::PermissionResolved { id });
        if self.asks.of(&session).is_empty() {
            self.set_state(&session, SessionState::Running);
        }
        answer
    }
}

/// The permission rules in the store, as the agent's rule gate reads them.
pub(crate) struct StoreRules<'a>(pub &'a Inner);

impl RuleStore for StoreRules<'_> {
    fn rules(&self) -> Vec<PermissionRule> {
        match self.0.store.all_permission_rules() {
            Ok(rules) => rules,
            Err(error) => {
                self.0.notice_error(&format!("Could not read the permission rules: {error:#}"));
                Vec::new()
            }
        }
    }

    fn add(&self, rule: PermissionRule) {
        match self.0.store.add_permission_rule(&rule) {
            Ok(()) => self.0.publish(Event::SettingsChanged),
            Err(error) => self.0.notice_error(&format!("Could not save the permission rule: {error:#}")),
        }
    }
}
