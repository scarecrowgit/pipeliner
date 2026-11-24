//! Long texts: an error with a long unbreakable path stays inside the runner card and the
//! panel, and the error texts (the failure callout, a question, a step's summary, a failed
//! wake-up's reason, a note) can be selected with the mouse and copied.

use gpui::{Bounds, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px};
use hub::api::{Actor, Event, IssueEvent, IssueEventKind, NodeStatus, Run, RunState};
use hub::model::IssueKey;
use ui::theme::LABEL;

use super::{NOW, Open, click, leak, open, open_with, shown};

/// A path of 200 characters with no place to break.
fn long_path() -> String {
    let mut path = String::from("/home/user/");
    while path.len() < 200 {
        path.push_str("pipeliner-mockups-screens-v2_");
    }
    path.truncate(200);
    path
}

/// The error the owner saw, with a 200-character path in it.
fn error() -> String {
    format!(
        "Merge mockup failed: `main` is checked out in {} with changes the merge would overwrite; nothing was merged \
         (`git reset --keep --quiet 937a0a362fc0fea6262662a4c74670004693c29e` failed (exit 128): fatal: Cannot do a keep \
         reset in the middle of a merge.)",
        long_path()
    )
}

/// Run-1427 changed by `edit`, sent as `RunChanged`.
fn change(t: &Open, cx: &mut VisualTestContext, edit: impl FnOnce(&mut Run)) {
    let found = t.api.with_world(|w| w.runs.iter().find(|r| r.id.as_str() == "run-1427").cloned());
    let Some(mut run) = found else { panic!("no run-1427") };
    edit(&mut run);
    t.api.with_world(|w| {
        if let Some(known) = w.runs.iter_mut().find(|r| r.id == run.id) {
            *known = run.clone();
        }
    });
    t.api.emit(Event::RunChanged { run });
    cx.run_until_parked();
}

fn fail(t: &Open, cx: &mut VisualTestContext) {
    change(t, cx, |run| {
        run.state = RunState::Failed;
        run.error = Some(error());
        run.finished = Some(NOW);
        for node in &mut run.nodes {
            if node.status == NodeStatus::Running {
                node.status = NodeStatus::Failed;
            }
        }
    });
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"))
}

fn assert_inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>, what: &str) {
    let slack = px(0.5);
    assert!(
        inner.left() >= outer.left() - slack
            && inner.right() <= outer.right() + slack
            && inner.top() >= outer.top() - slack
            && inner.bottom() <= outer.bottom() + slack,
        "{what} {inner:?} leaves {outer:?}"
    );
}

fn drag(cx: &mut VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

fn clipboard(cx: &mut VisualTestContext) -> String {
    cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default()
}

/// Drags from the start of the text drawn at `selector` to the end of its second line and
/// copies; returns the clipboard.
fn drag_copy(cx: &mut VisualTestContext, selector: &str) -> String {
    drag_copy_at(cx, selector, 0.)
}

/// [`drag_copy`] for a text `inset` px right of the element's left edge.
fn drag_copy_at(cx: &mut VisualTestContext, selector: &str, inset: f32) -> String {
    let b = bounds(cx, selector);
    let line = px(LABEL.line);
    let from = point(b.left() + px(inset + 0.5), b.top() + line / 2.);
    let to = point(b.right() - px(1.), b.top() + line * 1.5);
    drag(cx, from, to);
    cx.simulate_keystrokes("ctrl-c");
    clipboard(cx)
}

#[gpui::test]
fn a_long_error_stays_inside_the_runner_card(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    fail(&t, cx);
    let panel = bounds(cx, "issue-panel");
    let card = bounds(cx, "issue-runner-card");
    assert_inside(card, panel, "the runner card");
    for part in [
        "issue-runner-head",
        "issue-run-path",
        "issue-run-line",
        "issue-run-line-text",
        "issue-runner-triggers",
        "issue-runner-actions",
        "issue-prop-pipeline",
        "issue-open-runner",
        "issue-detach",
    ] {
        assert_inside(bounds(cx, part), card, part);
    }
    assert_eq!(bounds(cx, "issue-run-line-text").size.height, px(LABEL.line), "the status line is one line");
    let callout = bounds(cx, "issue-run-failure");
    assert_inside(callout, panel, "the failure callout");
    assert_inside(bounds(cx, "issue-run-failure-text"), callout, "the error");
    assert_inside(bounds(cx, "issue-resume"), callout, "Resume");
    assert!(bounds(cx, "issue-run-failure-text").size.height > px(LABEL.line * 3.), "the whole error wraps in the callout");
}

#[gpui::test]
fn the_failure_can_be_selected_and_copied(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    fail(&t, cx);
    let full = format!("Run failed · {}", error());
    let copied = drag_copy(cx, "issue-run-failure-text");
    assert!(copied.len() > "Run failed · Merge".len(), "a line and more is copied: {copied:?}");
    assert!(full.starts_with(&copied), "{copied:?} is the dragged start of the callout");

    // A click and Ctrl+A take the whole text.
    let b = bounds(cx, "issue-run-failure-text");
    cx.simulate_click(point(b.left() + px(4.), b.top() + px(LABEL.line / 2.)), Modifiers::none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx), full);
    assert!(shown(cx, "issue-resume"), "Resume stays beside the text");
}

#[gpui::test]
fn a_question_can_be_copied(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    let question = format!("Which merchant should I seed the refund for? The fixture lives in {}", long_path());
    let q = question.clone();
    change(&t, cx, move |run| {
        run.state = RunState::WaitingAnswer;
        run.nodes[2].status = NodeStatus::Waiting;
        run.nodes[2].question = Some(q);
    });
    let copied = drag_copy(cx, "issue-run-question-2-text");
    assert!(copied.len() > 10 && question.starts_with(&copied), "{copied:?}");
    assert_inside(bounds(cx, "issue-run-question-2"), bounds(cx, "issue-panel"), "the question callout");
}

#[gpui::test]
fn a_failed_step_shows_its_summary_and_any_step_unfolds(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    let long_name = "Reproducer-".repeat(20);
    let summary = format!("cargo test failed in {}", long_path());
    let (name, s) = (long_name.clone(), summary.clone());
    change(&t, cx, move |run| {
        run.state = RunState::Failed;
        run.nodes[0].name = name;
        run.nodes[0].status = NodeStatus::Failed;
        run.nodes[0].summary = s;
        run.nodes[1].summary = "Routed to the fixer".into();
    });
    let panel = bounds(cx, "issue-panel");
    assert_inside(bounds(cx, "issue-step-run-1427-0-cost"), panel, "a step's cost beside a long name");
    assert!(shown(cx, "issue-step-run-1427-0-summary"), "a failed step's summary is unfolded");
    assert_inside(bounds(cx, "issue-step-run-1427-0-summary"), panel, "the summary");
    // The summary starts after the dot's indent.
    let copied = drag_copy_at(cx, "issue-step-run-1427-0-summary", 6. + ui::theme::SPACE_2);
    assert!(copied.len() > 5 && summary.starts_with(&copied), "{copied:?}");

    let key = (t.panel.read_with(cx, |p, cx| p.latest_run(cx).map(|r| r.id.clone()))).unwrap_or_else(|| panic!("no run"));
    let open_of = |cx: &mut VisualTestContext| t.panel.read_with(cx, |p, _| p.texts.get(&format!("step-{key}-1")).is_some());
    assert!(!open_of(cx), "a step that did not fail starts folded");
    click(cx, "issue-step-run-1427-1");
    assert!(open_of(cx), "a click on the step unfolds its summary");
}

#[gpui::test]
fn feed_texts_stay_inside_and_can_be_copied(cx: &mut TestAppContext) {
    let note = format!("Could not push: {} is not a branch", long_path());
    let n = note.clone();
    let (t, cx) = open_with(cx, "PAY-1427", move |w| {
        let feed = w.issue_events.entry(IssueKey::new("PAY-1427")).or_default();
        let seq = feed.len() as u64 + 1;
        feed.push(IssueEvent { seq, at: NOW - 1_000, actor: Actor::System, kind: IssueEventKind::Note { text: n } });
    });
    let ix = t.api.with_world(|w| w.issue_events[&IssueKey::new("PAY-1427")].len()) - 1;
    let panel = bounds(cx, "issue-panel");
    let row = format!("issue-activity-{ix}");
    assert_inside(bounds(cx, &format!("{row}-text")), panel, "the note");
    let copied = drag_copy(cx, &format!("{row}-text"));
    assert!(copied.len() > 5 && note.starts_with(&copied), "{copied:?}");

    // The reason a wake-up failed, inside the comment that asked for it.
    let reason = "runtime laptop offline since 14:02 · reconnect it or retry";
    assert_inside(bounds(cx, "issue-activity-8"), panel, "the failed wake-up");
    assert_inside(bounds(cx, "issue-retry-wake-8"), bounds(cx, "issue-activity-8"), "Retry now");
    let b = bounds(cx, "issue-activity-8-text");
    drag(cx, point(b.left() + px(0.5), b.top() + px(LABEL.line / 2.)), point(b.right() - px(1.), b.bottom() - px(2.)));
    cx.simulate_keystrokes("ctrl-c");
    let copied = clipboard(cx);
    assert!(copied.len() > 5 && reason.starts_with(&copied), "{copied:?}");
}
