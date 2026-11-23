//! How runs move issues, and what moving an issue by hand sets off.

use model::{Issue, IssueStatus, Run, RunId, RunState, RunnableRef};

#[cfg(test)]
mod tests;

/// What a finished run left behind, as far as the issue is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunResult {
    /// The work was merged.
    Merged,
    /// A pull request is open and waits for review.
    PullRequestOpen,
    /// Nothing to show for it (an answer, an investigation).
    NoResult,
}

/// The status an issue takes when its latest run enters `run`.
///
/// Queued, running and waiting-for-an-answer runs put it In progress; a run waiting for a review
/// puts it In review. A finished run puts it Done when its work was merged, In review when it
/// left a pull request open, and leaves it alone otherwise. Failed, cancelled and interrupted
/// runs leave it where it is. Apply the result only when a run changes state: a person's manual
/// move stands until the next run event.
pub fn status_after_run(current: IssueStatus, run: &RunState, result: RunResult) -> IssueStatus {
    match run {
        RunState::Queued | RunState::Running | RunState::WaitingAnswer => IssueStatus::InProgress,
        RunState::WaitingReview => IssueStatus::InReview,
        RunState::Done => match result {
            RunResult::Merged => IssueStatus::Done,
            RunResult::PullRequestOpen => IssueStatus::InReview,
            RunResult::NoResult => current,
        },
        RunState::Failed | RunState::Cancelled | RunState::Interrupted => current,
    }
}

/// Whether a person may drag an issue from one status to another. Every move is allowed: the
/// person always wins over the runs.
pub fn can_move(_from: IssueStatus, _to: IssueStatus) -> bool {
    true
}

/// A follow-up of a manual move that the caller carries out or asks about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Start the attached runner on the issue.
    StartRunner {
        /// The runner attached under "Runs with".
        runner: RunnableRef,
        /// For the activity feed: "moved to Todo".
        cause: String,
    },
    /// Ask whether the open run should be cancelled before the move goes through.
    AskCancelRun {
        /// The run still open on the issue.
        run: RunId,
        /// The question, ready to show.
        question: String,
    },
}

/// What moving `issue` to `to` sets off, given its latest run (`None` when it has none).
///
/// Moving to Todo starts the attached runner when `triggers.on_todo` is set and no run is open
/// yet. Moving to Done or Backlog while a run is open asks whether to cancel it. Moving to the
/// status it already has sets off nothing. Paused or archived runners are the caller's to refuse:
/// ask `triggers::wake_intents` with `Happening::MovedTo` for that.
pub fn effects_of_move(issue: &Issue, to: IssueStatus, latest_run: Option<&Run>) -> Vec<Effect> {
    if issue.status == to {
        return Vec::new();
    }
    let open = latest_run.filter(|run| run.state.is_open());
    match (to, open) {
        (IssueStatus::Todo, None) => match (&issue.runner, issue.triggers.on_todo) {
            (Some(runner), true) => {
                vec![Effect::StartRunner { runner: runner.clone(), cause: "moved to Todo".to_string() }]
            }
            _ => Vec::new(),
        },
        (IssueStatus::Done | IssueStatus::Backlog, Some(run)) => vec![Effect::AskCancelRun {
            run: run.id.clone(),
            question: format!("Cancel the open run on {} before moving it to {}?", issue.key, to.label()),
        }],
        _ => Vec::new(),
    }
}
