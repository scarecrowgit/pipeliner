//! The event bus: every subscriber gets every event published after it subscribed.

use std::sync::{Mutex, MutexGuard, PoisonError};

use async_channel::{Receiver, Sender};
use model::Event;

/// Fan-out of [`Event`]s to any number of receivers. Publishing never blocks: channels are
/// unbounded, and a receiver that was dropped is forgotten on the next publish.
#[derive(Default)]
pub(crate) struct Bus {
    subscribers: Mutex<Vec<Sender<Event>>>,
}

impl Bus {
    /// A new receiver of every event from now on.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = async_channel::unbounded();
        lock(&self.subscribers).push(tx);
        rx
    }

    /// Sends `event` to every live subscriber.
    pub fn publish(&self, event: Event) {
        let mut subscribers = lock(&self.subscribers);
        subscribers.retain(|tx| tx.try_send(event.clone()).is_ok());
    }
}

/// A lock that survives a panic of another holder: everything behind the engine's locks is
/// plain state that stays consistent between statements.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
