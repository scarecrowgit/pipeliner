//! The executor: the state machine of one run and the threads that drive it.
//!
//! [`Machine`] is pure: it is fed what happened (a job ended, a person decided a review, the
//! budget was raised) together with the time, and answers with [`Effect`]s (start this job,
//! cancel that one, a review opened, the run changed). It owns no thread, does no IO and has no
//! clock. Its whole state is a [`Snapshot`], a serde value the engine stores after every step;
//! [`Machine::restore`] continues from one.
//!
//! [`Driver`] runs a machine on real threads: one worker per started job calling
//! [`NodeRunner::run`](crate::api::NodeRunner::run), with an [`Observer`] through which the
//! engine persists snapshots and hears about runs, reviews and log lines.
//!
//! Choices made where the contract is silent:
//!
//! - Only agent and orchestrator jobs count against `limits.max_parallel`; routers and actions
//!   start as soon as the item reaches them.
//! - A key never has two jobs at once. A job that was told to stop still reports its end; an
//!   instance that is to run again (after a resume or a budget raise) waits for that report.
//! - A node with `variants = 1` opens no scope: its instance has variant 0 and a review after it
//!   shows a single variant with index 1. Variants asked for inside an open scope are ignored.
//! - "Skip" finishes the review through the port [`PORT_SKIPPED`]: every node after the review is
//!   skipped, work on branches that do not pass through the review finishes, then the run is done.
//!   Nothing is discarded.
//! - A send-back beyond `limits.max_loops` is refused with [`ExecError::LoopLimit`]; the review
//!   stays open, so the run keeps waiting for a person. A `variants` count of 0 keeps the count.
//! - A node's `budget_usd` is a cap per instance (per variant and attempt). `raise_budget(usd)`
//!   allows `usd` more from what was spent, for the run and for instances stopped at their cap.
//! - While a person is needed the run says so even if other jobs are still working:
//!   `WaitingAnswer` wins over `WaitingReview`, which wins over `Running`.
//! - A job that continues earlier work of its instance (an answer, a raise, a resume) gets the
//!   instance's session in `Job::resume_session`.
//! - Cancelling or failing a run closes its open reviews (state `Skipped`); resuming builds
//!   them again under the same id. A run that is merely interrupted keeps them open.
//! - A spawned sub-agent's instance has the runner's id of the sub-agent as its `node`, and the
//!   variant and attempt of the instance that created it.
//! - The item enters at the source nodes of its own kind; when the graph has none of that kind,
//!   at its first source.

mod driver;
mod flow;
mod jobs;
mod machine;
mod plan;
mod review;
mod sendback;
mod state;
#[cfg(test)]
mod tests;

use std::fmt;

use model::{Review, ReviewId, Run, RunLogLine, RunState};

use crate::api::{InstanceKey, Job, NodeOutput};

pub use driver::{Driver, DriverHandle, Observer};
pub use machine::Machine;
pub use state::{Halt, Hold, Meta, PlannedRetry, ReviewRound, Snapshot, SpawnLink};

/// The port a review instance leaves through when a person chose "Skip". No edge starts there.
pub const PORT_SKIPPED: &str = "skipped";

/// What the machine asks of whoever drives it, in the order it must happen.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Run this job on a worker and report its end with [`Machine::job_finished`].
    Start(Box<Job>),
    /// Fire the cancel token of the job running under this key. Its result is still expected.
    CancelJob(InstanceKey),
    /// A review is waiting for a person.
    ReviewOpened(Review),
    /// A review was decided, or closed because its run stopped.
    ReviewClosed(Review),
    /// The run as it is now; sent once per step, after everything that changed it.
    Run(Box<Run>),
    /// A line for the run's log.
    Log(RunLogLine),
    /// An agent stopped to ask a person; [`Machine::answer`] continues it.
    Asked { key: InstanceKey, question: String },
    /// Hand this output to [`NodeRunner::discard`](crate::api::NodeRunner::discard): its
    /// worktree and branch are no longer part of the run.
    Discard(Box<NodeOutput>),
    /// The run reached `Done`, `Failed` or `Cancelled`. Jobs that were cancelled may still report.
    Finished(RunState),
}

/// Why the machine refused an input. The state is unchanged whenever one is returned.
#[derive(Clone, Debug, PartialEq)]
pub enum ExecError {
    /// `start` was called twice.
    AlreadyStarted,
    /// The run is done, failed or cancelled and does not take this input.
    RunClosed(RunState),
    /// `resume` on a run that is not stopped.
    NotStopped,
    /// No instance has this key.
    UnknownInstance(InstanceKey),
    /// The instance has no job in flight: a result or note that arrives twice or too late.
    NoJob(InstanceKey),
    /// The instance is not waiting for an answer.
    NotWaiting(InstanceKey),
    /// The review does not exist or is no longer open.
    ReviewNotOpen(ReviewId),
    /// The review has no variant with this index.
    UnknownVariant(u32),
    /// The review was already sent back `limits.max_loops` times; ship or skip it.
    LoopLimit(u32),
    /// The review names no node to send work back to, or one that is not upstream of it.
    NoSendBackTarget,
    /// A budget raise that is not a positive amount.
    BadAmount,
    /// The driver's thread is gone.
    DriverStopped,
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::AlreadyStarted => write!(f, "the run was already started"),
            ExecError::RunClosed(state) => write!(f, "the run is closed ({state:?})"),
            ExecError::NotStopped => write!(f, "the run is not stopped, there is nothing to resume"),
            ExecError::UnknownInstance(key) => write!(f, "no instance {}", show_key(key)),
            ExecError::NoJob(key) => write!(f, "no job is in flight for {}", show_key(key)),
            ExecError::NotWaiting(key) => write!(f, "{} is not waiting for an answer", show_key(key)),
            ExecError::ReviewNotOpen(id) => write!(f, "review {id} is not open"),
            ExecError::UnknownVariant(index) => write!(f, "the review has no variant {index}"),
            ExecError::LoopLimit(max) => write!(f, "the review was already sent back {max} times; ship or skip it"),
            ExecError::NoSendBackTarget => write!(f, "the review has no upstream node to send work back to"),
            ExecError::BadAmount => write!(f, "a budget raise must be a positive amount"),
            ExecError::DriverStopped => write!(f, "the run's driver has stopped"),
        }
    }
}

impl std::error::Error for ExecError {}

fn show_key(key: &InstanceKey) -> String {
    format!("{} (variant {}, attempt {})", key.node, key.variant, key.attempt)
}
