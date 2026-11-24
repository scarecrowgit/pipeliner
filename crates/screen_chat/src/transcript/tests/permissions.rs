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
fn reject_answers_with_an_empty_note_and_resolves(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, FIX, 4000.);
    click(cx, "perm-deny");
    let sent = answers(&open);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("Reject") && sent[0].contains("note: \"\""), "{sent:?}");
    assert!(!waiting(&open, cx));
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
