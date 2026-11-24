//! The scheduler: one engine thread that sleeps until the next due autopilot, fires it through
//! the same step as [`crate::api::IssueApi::run_autopilot`], and sweeps finished runs'
//! worktrees once a day.
//!
//! The engine's clock decides what is due; the thread's own waits are real time, capped at
//! [`MAX_WAIT`] so a clock that jumps (a laptop waking up, a test moving its `FixedClock`) is
//! noticed. Saving or deleting an autopilot, and moving a test clock with [`Wakeup::poke`],
//! wake it at once. The thread holds only a weak reference to the engine and ends when the
//! engine is dropped or shut down.

use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use anyhow::Result;
use issues::autopilot::Tick;
use model::Event;

use crate::bus::lock;
use crate::engine::{Engine, Inner};

/// Longest the thread sleeps before it looks at the clock again.
const MAX_WAIT: Duration = Duration::from_secs(60);
/// How often finished runs' worktrees are swept.
pub(crate) const SWEEP_EVERY_MS: i64 = 24 * 3_600_000;
/// Marker holding when the last sweep ran.
const SWEEP_MARKER: &str = "worktree-sweep:last";

#[derive(Default)]
struct WakeState {
    poked: bool,
    stopped: bool,
}

/// Wakes the scheduler thread early, or stops it.
#[derive(Default)]
pub(crate) struct Wakeup {
    state: Mutex<WakeState>,
    changed: Condvar,
}

impl Wakeup {
    /// Makes the thread look at the autopilots again now.
    pub fn poke(&self) {
        lock(&self.state).poked = true;
        self.changed.notify_all();
    }

    /// Ends the thread at its next wake-up.
    pub fn stop(&self) {
        lock(&self.state).stopped = true;
        self.changed.notify_all();
    }

    /// Waits up to `limit` for a poke; `false` once stopped.
    fn wait(&self, limit: Duration) -> bool {
        let mut state = lock(&self.state);
        if !state.poked && !state.stopped {
            state = self.changed.wait_timeout(state, limit).map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
        }
        state.poked = false;
        !state.stopped
    }
}

/// Starts the scheduler thread of `inner`.
pub(crate) fn start(inner: &Arc<Inner>) -> Result<()> {
    let weak: Weak<Inner> = Arc::downgrade(inner);
    let wakeup = inner.wakeup.clone();
    let handle = std::thread::Builder::new().name("scheduler".into()).spawn(move || {
        let mut next_sweep = i64::MIN;
        let mut last_error = String::new();
        loop {
            let wait = {
                let Some(inner) = weak.upgrade() else { return };
                if inner.is_shut_down() {
                    return;
                }
                let engine = Engine { inner };
                let now = engine.inner.now();
                if now >= next_sweep {
                    next_sweep = engine.scheduled_sweep(now, next_sweep == i64::MIN);
                }
                let next = match engine.fire_due() {
                    Ok(next) => {
                        last_error.clear();
                        next
                    }
                    Err(error) => {
                        let text = format!("Autopilots could not be checked: {error:#}");
                        if text != last_error {
                            engine.inner.notice_error(&text);
                            last_error = text;
                        }
                        None
                    }
                };
                let now = engine.inner.now();
                let at = next.map_or(next_sweep, |at| at.min(next_sweep));
                Duration::from_millis(u64::try_from(at.saturating_sub(now)).unwrap_or(0)).min(MAX_WAIT)
            };
            if !wakeup.wait(wait) {
                return;
            }
        }
    })?;
    lock(&inner.threads).push(handle);
    Ok(())
}

impl Engine {
    /// Fires every due autopilot (a tick missed by more than a day is skipped instead) and
    /// returns when the next one is due.
    pub(crate) fn fire_due(&self) -> Result<Option<i64>> {
        let inner = &self.inner;
        let now = inner.now();
        let offset = base::time::utc_offset(now);
        let all = inner.store.autopilots(None)?;
        let mut changed = false;
        for autopilot in &all {
            // An enabled schedule without a time (imported, or saved by an older build) gets one.
            if autopilot.next_run.is_none()
                && autopilot.enabled
                && let model::AutopilotTrigger::Schedule { schedule } = &autopilot.trigger
            {
                let mut fixed = autopilot.clone();
                fixed.next_run = Some(issues::autopilot::next_run(schedule, now, offset));
                inner.store.upsert_autopilot(&fixed)?;
                changed = true;
            }
        }
        for autopilot in issues::autopilot::due(&all, now) {
            match issues::autopilot::tick(autopilot, now, offset) {
                Tick::Fire => {
                    // Failures are recorded in `last_result`; the next one still fires.
                    let _ = self.fire_autopilot(autopilot.clone(), true);
                }
                Tick::Skip { next_run } => {
                    let mut skipped = autopilot.clone();
                    skipped.next_run = Some(next_run);
                    inner.store.upsert_autopilot(&skipped)?;
                    changed = true;
                }
            }
        }
        if changed {
            inner.publish(Event::AutopilotsChanged);
        }
        Ok(issues::autopilot::next_due(&inner.store.autopilots(None)?))
    }

    /// Sweeps worktrees when the last sweep is a day old (or on the first call after start);
    /// returns when the next one is due.
    fn scheduled_sweep(&self, now: i64, on_start: bool) -> i64 {
        let inner = &self.inner;
        let last = inner.store.marker(SWEEP_MARKER).ok().flatten().and_then(|v| v.parse::<i64>().ok());
        if let Some(last) = last.filter(|last| !on_start && now - last < SWEEP_EVERY_MS) {
            return last + SWEEP_EVERY_MS;
        }
        if let Err(error) = crate::sweep::sweep(inner, false) {
            inner.notice_error(&format!("Old worktrees could not be cleaned up: {error:#}"));
        }
        let _ = inner.store.set_marker(SWEEP_MARKER, &now.to_string());
        now + SWEEP_EVERY_MS
    }
}
