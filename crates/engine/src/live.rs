//! What is running right now: one entry per chat session with a turn in progress or messages
//! waiting, and the slot limit every turn passes through.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use base::cancel::CancelToken;
use model::SessionId;

use crate::api::UserMessage;
use crate::bus::lock;

/// How often a turn waiting for a slot looks at its cancel token.
const SLOT_POLL: Duration = Duration::from_millis(25);

#[derive(Default)]
struct LiveSession {
    /// A thread is driving the session.
    running: bool,
    cancel: Option<CancelToken>,
    /// Messages sent while the agent was busy, oldest first.
    queued: Vec<UserMessage>,
}

/// What [`Live::send`] did with a message.
pub(crate) enum Sent {
    /// Nothing was running: a turn starts with this message.
    Start { cancel: CancelToken, first: UserMessage },
    /// A turn is running; the message waits in the queue.
    Queued,
}

/// The running sessions, their cancel tokens and queues.
#[derive(Default)]
pub(crate) struct Live {
    sessions: Mutex<HashMap<SessionId, LiveSession>>,
}

impl Live {
    /// Queues `message`, or claims the session for a new turn when nothing runs. A session
    /// with older queued messages starts with the oldest; the rest stay queued for the loop.
    pub fn send(&self, id: &SessionId, message: UserMessage) -> Sent {
        let mut sessions = lock(&self.sessions);
        let live = sessions.entry(id.clone()).or_default();
        live.queued.push(message);
        if live.running {
            return Sent::Queued;
        }
        let cancel = CancelToken::new();
        live.running = true;
        live.cancel = Some(cancel.clone());
        let first = live.queued.remove(0);
        Sent::Start { cancel, first }
    }

    /// Claims the session for a turn that starts without a new message (retry, compact).
    /// `None` when a turn is running.
    pub fn claim(&self, id: &SessionId) -> Option<CancelToken> {
        let mut sessions = lock(&self.sessions);
        let live = sessions.entry(id.clone()).or_default();
        if live.running {
            return None;
        }
        let cancel = CancelToken::new();
        live.running = true;
        live.cancel = Some(cancel.clone());
        Some(cancel)
    }

    /// The oldest queued message, taken out of the queue; the session stays claimed.
    pub fn next(&self, id: &SessionId) -> Option<UserMessage> {
        let mut sessions = lock(&self.sessions);
        let live = sessions.get_mut(id)?;
        (!live.queued.is_empty()).then(|| live.queued.remove(0))
    }

    /// Releases the session, unless a message arrived since [`Live::next`] said there was
    /// none: then that message is returned and the session stays claimed.
    pub fn release(&self, id: &SessionId, deliver: bool) -> Option<UserMessage> {
        let mut sessions = lock(&self.sessions);
        let live = sessions.get_mut(id)?;
        if deliver && !live.queued.is_empty() {
            return Some(live.queued.remove(0));
        }
        live.running = false;
        live.cancel = None;
        None
    }

    /// Takes every queued message, oldest first, for the agent loop.
    pub fn take_queued(&self, id: &SessionId) -> Vec<UserMessage> {
        lock(&self.sessions).get_mut(id).map(|live| std::mem::take(&mut live.queued)).unwrap_or_default()
    }

    /// Puts messages back at the front of the queue (the loop could not store them).
    pub fn requeue(&self, id: &SessionId, messages: Vec<UserMessage>) {
        let mut sessions = lock(&self.sessions);
        let live = sessions.entry(id.clone()).or_default();
        let rest = std::mem::replace(&mut live.queued, messages);
        live.queued.extend(rest);
    }

    /// A copy of the queue.
    pub fn queued(&self, id: &SessionId) -> Vec<UserMessage> {
        lock(&self.sessions).get(id).map(|live| live.queued.clone()).unwrap_or_default()
    }

    /// Cancels the running turn, if there is one. Returns whether there was.
    pub fn cancel(&self, id: &SessionId) -> bool {
        match lock(&self.sessions).get(id).and_then(|live| live.cancel.clone()) {
            Some(cancel) => {
                cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancels every running turn.
    pub fn cancel_all(&self) {
        for live in lock(&self.sessions).values() {
            if let Some(cancel) = &live.cancel {
                cancel.cancel();
            }
        }
    }

    pub fn is_running(&self, id: &SessionId) -> bool {
        lock(&self.sessions).get(id).is_some_and(|live| live.running)
    }

    /// Sessions with a turn in progress.
    pub fn running(&self) -> Vec<SessionId> {
        lock(&self.sessions).iter().filter(|(_, live)| live.running).map(|(id, _)| id.clone()).collect()
    }

    /// Forgets a deleted session, cancelling what it runs.
    pub fn forget(&self, id: &SessionId) {
        if let Some(live) = lock(&self.sessions).remove(id)
            && let Some(cancel) = live.cancel
        {
            cancel.cancel();
        }
    }
}

/// The limit of agents working at once. The capacity is read on every acquire, so a change
/// in the settings applies to the next turn.
#[derive(Default)]
pub(crate) struct Slots {
    busy: Mutex<u32>,
    freed: Condvar,
}

/// A taken slot; dropping it frees the slot.
pub(crate) struct Slot<'a>(&'a Slots);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut busy = lock(&self.0.busy);
        *busy = busy.saturating_sub(1);
        self.0.freed.notify_all();
    }
}

impl Slots {
    /// Waits for a free slot. `None` when `cancel` fired first.
    pub fn acquire(&self, capacity: impl Fn() -> u32, cancel: &CancelToken) -> Option<Slot<'_>> {
        let mut busy = lock(&self.busy);
        loop {
            if cancel.is_cancelled() {
                return None;
            }
            if *busy < capacity().max(1) {
                *busy += 1;
                return Some(Slot(self));
            }
            busy = self.freed.wait_timeout(busy, SLOT_POLL).map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
        }
    }

    /// Slots taken right now.
    pub fn busy(&self) -> u32 {
        *lock(&self.busy)
    }

    /// Wakes waiting turns, after the capacity grew.
    pub fn notify(&self) {
        self.freed.notify_all();
    }
}
