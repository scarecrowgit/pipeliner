//! A cancellation flag that can be shared between threads and nested.
//!
//! A long job (an agent run, a build) holds a [`CancelToken`] and checks it between steps;
//! the UI holds a clone and calls [`CancelToken::cancel`]. A job that starts sub-jobs gives
//! each a [`CancelToken::child`], so stopping the parent stops them all while stopping one
//! child leaves its siblings alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long [`CancelToken::sleep`] sleeps between looks at the flag.
const STEP: Duration = Duration::from_millis(20);

#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    parent: Option<CancelToken>,
}

/// A shared "stop now" flag. Cloning gives another handle to the same flag.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

impl CancelToken {
    /// A token that is not cancelled and has no parent.
    pub fn new() -> Self {
        Self::default()
    }

    /// A token that reads as cancelled once it or any ancestor is cancelled.
    ///
    /// Cancelling the child does not cancel `self`.
    pub fn child(&self) -> CancelToken {
        CancelToken { inner: Arc::new(Inner { flag: AtomicBool::new(false), parent: Some(self.clone()) }) }
    }

    /// Cancels this token and every token below it. Safe to call more than once.
    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
    }

    /// Whether this token or any ancestor has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        let mut token = self;
        loop {
            if token.inner.flag.load(Ordering::SeqCst) {
                return true;
            }
            match &token.inner.parent {
                Some(parent) => token = parent,
                None => return false,
            }
        }
    }

    /// Sleeps for `dur` in small steps. Returns `false` as soon as the token is cancelled,
    /// `true` when the whole time passed. A token that is already cancelled returns at once.
    pub fn sleep(&self, dur: Duration) -> bool {
        let end = Instant::now().checked_add(dur);
        loop {
            if self.is_cancelled() {
                return false;
            }
            let left = match end {
                Some(end) => end.saturating_duration_since(Instant::now()),
                None => STEP,
            };
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(STEP));
        }
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken").field("cancelled", &self.is_cancelled()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_clear_and_cancels_once_and_again() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn clones_share_the_flag() {
        let a = CancelToken::new();
        let b = a.clone();
        b.cancel();
        assert!(a.is_cancelled());
    }

    #[test]
    fn parent_cancels_children_at_any_depth() {
        let root = CancelToken::new();
        let child = root.child();
        let grandchild = child.child();
        let sibling = root.child();
        assert!(!grandchild.is_cancelled());
        root.cancel();
        assert!(child.is_cancelled());
        assert!(grandchild.is_cancelled());
        assert!(sibling.is_cancelled());
    }

    #[test]
    fn child_does_not_cancel_parent_or_siblings() {
        let root = CancelToken::new();
        let a = root.child();
        let b = root.child();
        let under_a = a.child();
        a.cancel();
        assert!(a.is_cancelled());
        assert!(under_a.is_cancelled());
        assert!(!root.is_cancelled());
        assert!(!b.is_cancelled());
    }

    #[test]
    fn child_of_cancelled_token_starts_cancelled() {
        let root = CancelToken::new();
        root.cancel();
        assert!(root.child().is_cancelled());
    }

    #[test]
    fn sleep_runs_full_time() {
        let token = CancelToken::new();
        let start = Instant::now();
        assert!(token.sleep(Duration::from_millis(60)));
        assert!(start.elapsed() >= Duration::from_millis(60));
        assert!(token.sleep(Duration::ZERO));
    }

    #[test]
    fn sleep_returns_at_once_when_already_cancelled() {
        let token = CancelToken::new();
        token.cancel();
        let start = Instant::now();
        assert!(!token.sleep(Duration::from_secs(30)));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!token.sleep(Duration::ZERO));
    }

    #[test]
    fn sleep_wakes_early_on_cancel_from_another_thread() {
        let token = CancelToken::new();
        let remote = token.child();
        let handle = std::thread::spawn({
            let token = token.clone();
            move || {
                std::thread::sleep(Duration::from_millis(60));
                token.cancel();
            }
        });
        let start = Instant::now();
        assert!(!remote.sleep(Duration::from_secs(30)));
        assert!(start.elapsed() < Duration::from_secs(5));
        handle.join().expect("canceller thread");
    }

    #[test]
    fn huge_sleep_does_not_overflow() {
        let token = CancelToken::new();
        token.cancel();
        assert!(!token.sleep(Duration::MAX));
    }
}
