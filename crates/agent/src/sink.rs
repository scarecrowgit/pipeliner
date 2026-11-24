//! Where a run's events go.

use std::sync::{Mutex, MutexGuard, PoisonError};

use model::{AgentEvent, SessionId};

/// Receives every [`AgentEvent`] of a run, in order, with the session it belongs to: the
/// run's own session, or a sub-agent's. With sub-agents running in parallel, events of
/// different sessions interleave; those of one session stay in order.
///
/// What is durable arrives as `MessageCommitted`. Deltas and tool calls announced since the
/// last `TurnStarted` are provisional: a `Retrying` or the end of the run without a committed
/// message means they were dropped, and a call that was started but never became ready gets
/// no `ToolFinished`.
///
/// `event` is called while the run holds a lock, from the run's thread and from its
/// sub-agents' threads: it must return quickly and must not call back into the run.
pub trait AgentSink: Send {
    fn event(&mut self, session: &SessionId, event: AgentEvent);
}

impl<F: FnMut(&SessionId, AgentEvent) + Send> AgentSink for F {
    fn event(&mut self, session: &SessionId, event: AgentEvent) {
        self(session, event);
    }
}

/// A sink that keeps everything, for tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventLog {
    pub events: Vec<(SessionId, AgentEvent)>,
}

impl EventLog {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// The events of one session, in order.
    pub fn of(&self, session: &SessionId) -> Vec<&AgentEvent> {
        self.events.iter().filter(|(s, _)| s == session).map(|(_, e)| e).collect()
    }
}

impl AgentSink for EventLog {
    fn event(&mut self, session: &SessionId, event: AgentEvent) {
        self.events.push((session.clone(), event));
    }
}

/// The caller's sink behind a lock, so a parent and its sub-agents' threads can all write.
pub(crate) struct SharedSink<'s> {
    inner: Mutex<&'s mut dyn AgentSink>,
}

impl<'s> SharedSink<'s> {
    pub fn new(sink: &'s mut dyn AgentSink) -> Self {
        Self { inner: Mutex::new(sink) }
    }
}

/// Something events can be sent to from any thread of a run.
pub(crate) trait Emit: Sync {
    fn emit(&self, session: &SessionId, event: AgentEvent);
}

impl Emit for SharedSink<'_> {
    fn emit(&self, session: &SessionId, event: AgentEvent) {
        lock(&self.inner).event(session, event);
    }
}

/// A lock that survives a panic of another holder: the data behind it is plain state.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
