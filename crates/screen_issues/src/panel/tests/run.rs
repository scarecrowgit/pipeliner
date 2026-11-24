//! The run section: it follows `RunChanged` (running, waiting for a review, done), shows a
//! question and answers it, offers Resume after a failure, and folds the earlier runs.

use gpui::{TestAppContext, VisualTestContext};
use hub::api::{Event, NodeId, NodeStatus, Run, RunId, RunState};
use hub::{InboxFocus, Route};

use super::{Open, calls, click, open, open_with, shown};

/// Run-1427 as the fixture holds it, changed by `edit`, sent as `RunChanged`.
fn change(t: &Open, cx: &mut VisualTestContext, edit: impl FnOnce(&mut Run)) {
    let mut run = t.api.with_world(|w| w.runs.iter().find(|r| r.id.as_str() == "run-1427").cloned()).unwrap_or_else(|| panic!("no run-1427"));
    edit(&mut run);
    t.api.with_world(|w| {
        if let Some(known) = w.runs.iter_mut().find(|r| r.id == run.id) {
            *known = run.clone();
        }
    });
    t.api.emit(Event::RunChanged { run });
    cx.run_until_parked();
}

#[gpui::test]
fn the_run_section_follows_run_changes(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    assert!(shown(cx, "issue-run-run-1427"), "the latest run has a section");
    assert!(shown(cx, "issue-step-run-1427-2"), "its steps are listed");
    assert!(!shown(cx, "issue-run-review"));

    change(&t, cx, |run| {
        run.state = RunState::WaitingReview;
        for node in &mut run.nodes {
            node.status = NodeStatus::Done;
        }
    });
    assert!(shown(cx, "issue-run-review"), "a waiting review links to the Inbox");

    change(&t, cx, |run| run.state = RunState::Done);
    let state = t.panel.read_with(cx, |p, cx| p.latest_run(cx).map(|r| r.state));
    assert_eq!(state, Some(RunState::Done), "the section shows the run as it is now");
    assert!(shown(cx, "issue-start"), "with the run done, the runner can start again");
}

#[gpui::test]
fn the_review_link_opens_the_review_in_the_inbox(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    change(&t, cx, |run| run.state = RunState::WaitingReview);
    click(cx, "issue-run-review");
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(route, Route::Inbox { item: Some(InboxFocus::Review("review-1427".into())) });
}

#[gpui::test]
fn a_question_is_shown_and_answered_in_place(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    change(&t, cx, |run| {
        run.state = RunState::WaitingAnswer;
        run.nodes[2].status = NodeStatus::Waiting;
        run.nodes[2].question = Some("Which merchant should I seed the 420-item refund for?".into());
    });
    assert!(shown(cx, "issue-run-question-2"));
    click(cx, "issue-run-answer-field-2");
    cx.simulate_input("acme-test");
    click(cx, "issue-run-answer-send-2");
    assert_eq!(calls(&t.api, "answer_run"), ["run-1427 repro acme-test"]);
}

#[gpui::test]
fn a_run_out_of_budget_can_be_raised(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    change(&t, cx, |run| {
        run.state = RunState::WaitingAnswer;
        run.nodes[2].status = NodeStatus::Waiting;
    });
    assert!(!shown(cx, "issue-run-budget"), "a run waiting with no question is not guessed to be over budget");
    // The engine says why the run waits.
    t.api.emit(Event::RunWaiting { run: RunId::new("run-1427"), waiting: Some(hub::api::RunWait::Budget) });
    cx.run_until_parked();
    assert!(shown(cx, "issue-run-budget"));
    click(cx, "issue-run-raise");
    assert_eq!(calls(&t.api, "raise_run_budget"), ["run-1427 1"]);
}

#[gpui::test]
fn a_failed_run_offers_resume(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    change(&t, cx, |run| {
        run.state = RunState::Failed;
        run.error = Some("gateway sandbox refused the API key · rotate it in Settings".into());
    });
    assert!(shown(cx, "issue-run-failure"));
    click(cx, "issue-resume");
    assert_eq!(calls(&t.api, "resume_run"), ["run-1427"]);
}

#[gpui::test]
fn earlier_runs_are_folded_until_opened(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, "PAY-1427", |w| {
        let Some(mut first) = w.runs.iter().find(|r| r.id.as_str() == "run-1427").cloned() else { return };
        first.id = RunId::new("run-1427-a");
        first.number = 1;
        first.state = RunState::Done;
        first.started -= 3_600_000;
        first.finished = Some(first.started + 120_000);
        for node in &mut first.nodes {
            node.status = NodeStatus::Done;
            node.node = NodeId::new(node.node.as_str());
        }
        w.runs.insert(0, first);
    });
    assert!(shown(cx, "issue-run-run-1427-a"), "Run 1 on this issue is listed");
    assert!(!shown(cx, "issue-step-run-1427-a-0"), "folded");
    click(cx, "issue-run-run-1427-a");
    assert!(shown(cx, "issue-step-run-1427-a-0"), "unfolded");
}
