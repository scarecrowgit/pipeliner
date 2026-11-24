//! What is running right now: one entry per chat session with a turn in progress or messages
//! waiting, and the slot limit every turn passes through. Every change of a queue is handed to
//! a saver (the store), so queued messages survive a restart.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
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
    queued: Vec<Queued>,
    /// Tags of the messages the turn in progress took in, in the order it took them.
    consumed: Vec<String>,
}

/// A queued message and the tag it was sent with, if any: a tag says which turn took the
/// message in, whatever its text (an issue's comment owes its issue the answer of that turn).
#[derive(Clone)]
struct Queued {
    message: UserMessage,
    tag: Option<String>,
}

impl Queued {
    fn plain(message: UserMessage) -> Self {
        Self { message, tag: None }
    }
}

impl LiveSession {
    /// Takes the oldest queued message as the turn's, noting its tag.
    fn take_first(&mut self) -> UserMessage {
        let first = self.queued.remove(0);
        self.consumed.extend(first.tag);
        first.message
    }

    fn messages(&self) -> Vec<UserMessage> {
        self.queued.iter().map(|q| q.message.clone()).collect()
    }
}

/// What [`Live::send`] did with a message.
pub(crate) enum Sent {
    /// Nothing was running: a turn starts with this message.
    Start { cancel: CancelToken, first: UserMessage },
    /// A turn is running; the message waits in the queue.
    Queued,
}

/// Keeps a session's queue somewhere that outlives the process; called with the whole queue
/// after every change, under the registry's lock so saves land in order.
pub(crate) type QueueSaver = Box<dyn Fn(&SessionId, &[UserMessage]) + Send + Sync>;

/// The running sessions, their cancel tokens and queues.
#[derive(Default)]
pub(crate) struct Live {
    sessions: Mutex<HashMap<SessionId, LiveSession>>,
    saver: OnceLock<QueueSaver>,
}

impl Live {
    /// Sets where queues are kept. Only the first call counts.
    pub fn save_queues_with(&self, saver: QueueSaver) {
        let _ = self.saver.set(saver);
    }

    /// Puts back a queue kept from before a restart, behind anything queued since. Not saved
    /// again: it is what the saver holds.
    pub fn restore(&self, id: &SessionId, messages: Vec<UserMessage>) {
        let mut sessions = lock(&self.sessions);
        let live = sessions.entry(id.clone()).or_default();
        let newer = std::mem::replace(&mut live.queued, messages.into_iter().map(Queued::plain).collect());
        live.queued.extend(newer);
    }

    fn save(&self, id: &SessionId, live: &LiveSession) {
        if let Some(saver) = self.saver.get() {
            saver(id, &live.messages());
        }
    }

    /// Queues `message`, or claims the session for a new turn when nothing runs. A session
    /// with older queued messages starts with the oldest; the rest stay queued for the loop.
    /// A `tag` comes back from [`Live::take_consumed`] once a turn took the message in.
    pub fn send_tagged(&self, id: &SessionId, message: UserMessage, tag: Option<String>) -> Sent {
        let mut sessions = lock(&self.sessions);
        let live = sessions.entry(id.clone()).or_default();
        live.queued.push(Queued { message, tag });
        if live.running {
            self.save(id, live);
            return Sent::Queued;
        }
        let cancel = CancelToken::new();
        live.running = true;
        live.cancel = Some(cancel.clone());
        let first = live.take_first();
        self.save(id, live);
        Sent::Start { cancel, first }
    }

    /// [`Live::send_tagged`] without a tag.
    #[cfg(test)]
    pub fn send(&self, id: &SessionId, message: UserMessage) -> Sent {
        self.send_tagged(id, message, None)
    }

    /// The tags of the messages the turn that just ended took in; the next turn starts with
    /// none.
    pub fn take_consumed(&self, id: &SessionId) -> Vec<String> {
        lock(&self.sessions).get_mut(id).map(|live| std::mem::take(&mut live.consumed)).unwrap_or_default()
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
        if live.queued.is_empty() {
            return None;
        }
        let next = live.take_first();
        self.save(id, live);
        Some(next)
    }

    /// Releases the session, unless a message arrived since [`Live::next`] said there was
    /// none: then that message is returned and the session stays claimed.
    pub fn release(&self, id: &SessionId, deliver: bool) -> Option<UserMessage> {
        let mut sessions = lock(&self.sessions);
        let live = sessions.get_mut(id)?;
        if deliver && !live.queued.is_empty() {
            let next = live.take_first();
            self.save(id, live);
            return Some(next);
        }
        live.running = false;
        live.cancel = None;
        None
    }

    /// Takes every queued message, oldest first, for the agent loop.
    pub fn take_queued(&self, id: &SessionId) -> Vec<UserMessage> {
        let mut sessions = lock(&self.sessions);
        let Some(live) = sessions.get_mut(id) else { return Vec::new() };
        let taken = std::mem::take(&mut live.queued);
        if !taken.is_empty() {
            self.save(id, live);
        }
        taken
            .into_iter()
            .map(|q| {
                live.consumed.extend(q.tag);
                q.message
            })
            .collect()
    }

    /// Puts messages back at the front of the queue (the loop could not store them).
    pub fn requeue(&self, id: &SessionId, messages: Vec<UserMessage>) {
        let mut sessions = lock(&self.sessions);
        let live = sessions.entry(id.clone()).or_default();
        let rest = std::mem::replace(&mut live.queued, messages.into_iter().map(Queued::plain).collect());
        live.queued.extend(rest);
        self.save(id, live);
    }

    /// A copy of the queue.
    pub fn queued(&self, id: &SessionId) -> Vec<UserMessage> {
        lock(&self.sessions).get(id).map(LiveSession::messages).unwrap_or_default()
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

    /// Forgets a deleted session, cancelling what it runs. Its kept queue goes with the
    /// session's row.
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
