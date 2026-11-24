//! The permission question on its tool row: each answer reaches the command, and the row
//! leaves the waiting state once the request is resolved.

use gpui::{TestAppContext, VisualTestContext};
use hub::transcript::{BlockKind, ToolState};

use super::{FIX, Open, click, drawn, open, settle};

/// The `bun test` call of the mockup chat.
const CALL_ROW: usize = 10;

fn answers(open: &Open) -> Vec<String> {
    open.api.recorded().into_iter().filter(|command| command.name == "answer_permission").map(|c| c.detail).collect()
}

fn waiting(open: &Open, cx: &mut VisualTestContext) -> bool {
    open.view.read_with(cx, |view, _| view.links.pending_on(CALL_ROW).is_some())
}

#[gpui::test]
fn allow_once_answers_and_the_row_runs_the_command(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    assert!(waiting(&open, cx));
    click(cx, "perm-allow");
    assert_eq!(answers(&open), ["perm-1423-bun-test AllowOnce"]);
    // The fixture resolves the request and plays the rest of the turn at once.
    assert!(!waiting(&open, cx));
    assert!(drawn(cx, "tool-t8".into()), "the row shows the command again");
    let state = open.view.read_with(cx, |view, cx| match &view.model.read(cx).blocks()[CALL_ROW].kind {
        BlockKind::Tool(tool) => Some(tool.state),
        _ => None,
    });
    assert_eq!(state, Some(ToolState::Finished { is_error: false }));
}

#[gpui::test]
fn always_allow_sends_the_rule_of_the_request(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    click(cx, "perm-always");
    let sent = answers(&open);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("AllowAlways") && sent[0].contains("bun test"), "{sent:?}");
    assert!(!waiting(&open, cx));
}

#[gpui::test]
fn reject_opens_a_note_field_and_sends_the_note(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    click(cx, "perm-deny");
    assert!(open.view.read_with(cx, |view, _| view.is_rejecting()), "Reject asks for a note first");
    assert!(answers(&open).is_empty());
    assert!(drawn(cx, "perm-reject-send".into()) && drawn(cx, "perm-reject-cancel".into()));

    click(cx, "perm-reject-cancel");
    assert!(!open.view.read_with(cx, |view, _| view.is_rejecting()));
    assert!(waiting(&open, cx), "Cancel keeps the question");

    click(cx, "perm-deny");
    let input = open.view.read_with(cx, |view, _| view.rejecting.as_ref().map(|r| r.input.clone()));
    let Some(input) = input else { panic!("the note field is open") };
    input.update(cx, |input, cx| input.set_text("run it in CI instead", cx));
    click(cx, "perm-reject-send");
    let sent = answers(&open);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Reject") && sent[0].contains("run it in CI instead"), "{sent:?}");
    assert!(!waiting(&open, cx));
}

#[gpui::test]
fn enter_in_the_note_field_rejects(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    let started = open.view.update_in(cx, |view, window, cx| view.start_reject(window, cx));
    assert!(started);
    settle(cx);
    cx.simulate_input("not on my laptop");
    cx.simulate_keystrokes("enter");
    settle(cx);
    let sent = answers(&open);
    assert!(sent.len() == 1 && sent[0].contains("not on my laptop"), "{sent:?}");
}

#[gpui::test]
fn the_keyboard_methods_answer_the_newest_question(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    let answered = open.view.update(cx, |view, cx| view.reject_pending("run it in CI instead", cx));
    assert!(answered);
    settle(cx);
    assert!(answers(&open)[0].contains("run it in CI instead"));
    let again = open.view.update(cx, |view, cx| view.allow_pending(cx));
    assert!(!again, "nothing is pending any more");
}

#[gpui::test]
fn in_a_narrow_column_the_answers_stay_inside_the_banner(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    cx.simulate_resize(gpui::size(gpui::px(560.), gpui::px(4000.)));
    settle(cx);
    settle(cx);
    let narrow = open.view.read_with(cx, |view, _| view.column_width);
    assert!(narrow < 600., "the column follows the window: {narrow}");
    let row = cx.debug_bounds("tool-t8").unwrap_or_else(|| panic!("the question is drawn"));
    let deny = cx.debug_bounds("perm-deny").unwrap_or_else(|| panic!("Reject is drawn"));
    assert!(deny.right() <= row.right(), "Reject {deny:?} stays in the banner {row:?}");
}

#[test]
fn a_long_rule_is_cut_in_the_always_button() {
    let world = hub::fixtures::World::acme(super::NOW);
    let Some(mut request) = world.permissions.first().cloned() else { panic!("the fixture has a pending request") };
    let Some(rule) = request.always_rule.as_mut() else { panic!("the request offers a rule") };
    rule.pattern = "npm run test:integration -- --runInBand --coverage --verbose".into();
    let label = crate::transcript::permission::always_label(&request).unwrap_or_default();
    assert_eq!(label, "Always allow npm run test:integration -- --r…");
}
