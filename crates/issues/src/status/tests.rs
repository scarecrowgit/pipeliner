use model::{IssueStatus, RunId, RunState, RunnableRef};

use super::*;
use crate::testing::{issue, run};

const ALL_STATES: [RunState; 8] = [
    RunState::Queued,
    RunState::Running,
    RunState::WaitingReview,
    RunState::WaitingAnswer,
    RunState::Done,
    RunState::Failed,
    RunState::Cancelled,
    RunState::Interrupted,
];

#[test]
fn working_runs_put_the_issue_in_progress() {
    for from in IssueStatus::ALL {
        for state in [RunState::Queued, RunState::Running, RunState::WaitingAnswer] {
            assert_eq!(status_after_run(from, &state, RunResult::NoResult), IssueStatus::InProgress);
        }
    }
}

#[test]
fn a_review_puts_it_in_review() {
    for from in IssueStatus::ALL {
        assert_eq!(status_after_run(from, &RunState::WaitingReview, RunResult::NoResult), IssueStatus::InReview);
    }
}

#[test]
fn done_depends_on_what_the_run_left() {
    let s = |r| status_after_run(IssueStatus::InProgress, &RunState::Done, r);
    assert_eq!(s(RunResult::Merged), IssueStatus::Done);
    assert_eq!(s(RunResult::PullRequestOpen), IssueStatus::InReview);
    assert_eq!(s(RunResult::NoResult), IssueStatus::InProgress);
    assert_eq!(status_after_run(IssueStatus::Todo, &RunState::Done, RunResult::NoResult), IssueStatus::Todo);
}

#[test]
fn failed_cancelled_and_interrupted_leave_it_alone() {
    for from in IssueStatus::ALL {
        for state in [RunState::Failed, RunState::Cancelled, RunState::Interrupted] {
            for result in [RunResult::Merged, RunResult::PullRequestOpen, RunResult::NoResult] {
                assert_eq!(status_after_run(from, &state, result), from);
            }
        }
    }
}

#[test]
fn every_move_is_allowed() {
    for from in IssueStatus::ALL {
        for to in IssueStatus::ALL {
            assert!(can_move(from, to));
        }
    }
}

#[test]
fn moving_to_todo_starts_the_attached_runner() {
    let mut i = issue(IssueStatus::Backlog);
    i.runner = Some(RunnableRef::agent("fixer"));
    i.triggers.on_todo = true;
    assert_eq!(
        effects_of_move(&i, IssueStatus::Todo, None),
        vec![Effect::StartRunner { runner: RunnableRef::agent("fixer"), cause: "moved to Todo".into() }]
    );
    // A finished earlier run does not block a new start.
    let finished = run("r1", RunState::Done);
    assert_eq!(effects_of_move(&i, IssueStatus::Todo, Some(&finished)).len(), 1);
}

#[test]
fn todo_without_trigger_runner_or_with_open_run_starts_nothing() {
    let mut i = issue(IssueStatus::Backlog);
    i.runner = Some(RunnableRef::agent("fixer"));
    assert!(effects_of_move(&i, IssueStatus::Todo, None).is_empty());
    i.triggers.on_todo = true;
    let open = run("r1", RunState::Running);
    assert!(effects_of_move(&i, IssueStatus::Todo, Some(&open)).is_empty());
    i.runner = None;
    assert!(effects_of_move(&i, IssueStatus::Todo, None).is_empty());
}

#[test]
fn closing_or_shelving_with_an_open_run_asks() {
    let i = issue(IssueStatus::InProgress);
    for state in [RunState::Queued, RunState::Running, RunState::WaitingReview, RunState::WaitingAnswer, RunState::Interrupted] {
        let open = run("r7", state);
        for to in [IssueStatus::Done, IssueStatus::Backlog] {
            let effects = effects_of_move(&i, to, Some(&open));
            assert!(matches!(&effects[..], [Effect::AskCancelRun { run, question }]
                if run == &RunId::new("r7") && question.contains("PAY-1") && question.contains(to.label())), "{state:?} {to:?}");
        }
    }
}

#[test]
fn closing_without_an_open_run_asks_nothing() {
    let i = issue(IssueStatus::InProgress);
    for state in [RunState::Done, RunState::Failed, RunState::Cancelled] {
        let r = run("r1", state);
        assert!(effects_of_move(&i, IssueStatus::Done, Some(&r)).is_empty());
    }
    assert!(effects_of_move(&i, IssueStatus::Backlog, None).is_empty());
}

#[test]
fn other_moves_with_an_open_run_ask_nothing() {
    let i = issue(IssueStatus::Todo);
    let open = run("r1", RunState::Running);
    for to in [IssueStatus::InProgress, IssueStatus::InReview] {
        assert!(effects_of_move(&i, to, Some(&open)).is_empty());
    }
    assert!(ALL_STATES.iter().all(|s| effects_of_move(&i, IssueStatus::Todo, Some(&run("r", *s))).is_empty()));
}
