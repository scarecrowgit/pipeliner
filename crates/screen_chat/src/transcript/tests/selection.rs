//! Text that is not a message can be selected with the mouse and copied: a command's output,
//! a tool's text, an error, what a permission question wants to run; long unbreakable text
//! wraps inside its row; rows keep their measured height while a tool's output grows.

use gpui::{Bounds, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px};
use hub::model::{AgentEvent, ToolClass, ToolDisplay};
use ui::theme::{LABEL, MONO, SPACE_1_5, SPACE_2, SPACE_3};

use super::{FIX, Open, counts, emit, open, selector, settle};

const CHAT: &str = "chat-split-checkout";

fn bounds(cx: &mut VisualTestContext, name: &str) -> Bounds<Pixels> {
    cx.debug_bounds(selector(name.to_string())).unwrap_or_else(|| panic!("`{name}` was not drawn"))
}

/// Presses at `from`, moves to `to` with the button down and lets go.
fn drag(cx: &mut VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    settle(cx);
}

fn copy(cx: &mut VisualTestContext) -> Option<String> {
    cx.simulate_keystrokes("ctrl-c");
    cx.read_from_clipboard().and_then(|item| item.text())
}

/// Drags over the first line of the text that starts `inset` inside `name`, from its first
/// character to past its end.
fn select_first_line(cx: &mut VisualTestContext, name: &str, inset: (f32, f32), line: f32) {
    let b = bounds(cx, name);
    let y = b.top() + px(inset.1 + line / 2.);
    drag(cx, point(b.left() + px(inset.0 + 1.), y), point(b.right() - px(inset.0 + 1.), y));
}

/// A command that runs with `output` so far.
fn command(output: &str) -> ToolDisplay {
    ToolDisplay::Command { command: "bun test src/charges".into(), output: output.into(), exit_code: None }
}

/// Starts a turn with the `bash` call `id` showing `display`; the row's index.
fn run(open: &Open, cx: &mut VisualTestContext, id: &str, display: ToolDisplay) -> usize {
    emit(
        open,
        cx,
        CHAT,
        vec![
            AgentEvent::TurnStarted,
            AgentEvent::ToolCallStarted { id: id.into(), name: "bash".into() },
            AgentEvent::ToolCallReady { id: id.into(), name: "bash".into(), class: ToolClass::Execute, display },
        ],
    );
    counts(open, cx).1 - 1
}

#[gpui::test]
fn a_commands_output_can_be_selected_and_copied(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, CHAT, 3000.);
    let index = run(&open, cx, "c", command("alpha one\nbeta two\ngamma three\n"));
    let id = open.view.read_with(cx, |view, cx| view.model.read(cx).blocks()[index].id);
    let name = format!("tool-output-{index}");
    select_first_line(cx, &name, (SPACE_3, SPACE_1_5), MONO.line);
    assert_eq!(copy(cx).as_deref(), Some("alpha one"));
    // The drag selected; it did not fold the output.
    assert_eq!(open.view.read_with(cx, |view, _| view.row_ui(id).open), None);
    assert!(cx.debug_bounds(selector(name.clone())).is_some());
    // The command itself can be selected too.
    select_first_line(cx, &format!("tool-command-{index}"), (0., (36. - MONO.line) / 2.), MONO.line);
    assert_eq!(copy(cx).as_deref(), Some("bun test src/charges"));
    // The fold button still folds.
    super::click(cx, &format!("toggle-output-{id}"));
    assert_eq!(open.view.read_with(cx, |view, _| view.row_ui(id).open), Some(false));
}

#[gpui::test]
fn a_question_cards_text_can_be_selected_and_copied(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, CHAT, 3000.);
    let body = "Which retry budget should the checkout use?\n\
                The old one retried five times.\n\
                The new one stops after two.\n\
                Answer with a number.";
    let display = ToolDisplay::Text { title: "Question".into(), body: body.into() };
    let index = run(&open, cx, "q", display.clone());
    emit(&open, cx, CHAT, vec![AgentEvent::ToolFinished { id: "q".into(), display, is_error: false }]);
    let name = format!("tool-text-{index}");
    select_first_line(cx, &name, (SPACE_3, SPACE_2), LABEL.line);
    assert_eq!(copy(cx).as_deref(), Some("Which retry budget should the checkout use?"));
    // Select-all takes the whole text, line breaks included.
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(copy(cx).as_deref(), Some(body));
}

#[gpui::test]
fn a_failed_turns_error_can_be_copied(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, CHAT, 3000.);
    emit(&open, cx, CHAT, vec![AgentEvent::TurnStarted, AgentEvent::Failed { error: "rate limited by the gateway".into() }]);
    let index = counts(&open, cx).1 - 1;
    select_first_line(cx, &format!("failure-text-{index}"), (0., 0.), LABEL.line);
    assert_eq!(copy(cx).as_deref(), Some("rate limited by the gateway"));
}

#[gpui::test]
fn what_a_permission_question_runs_can_be_copied(cx: &mut TestAppContext) {
    let (_open, cx) = open(cx, FIX, 4000.);
    select_first_line(cx, "perm-call", (0., 0.), MONO.line);
    let copied = copy(cx).unwrap_or_default();
    assert!(copied.starts_with("$ bun test"), "{copied:?}");
}

#[gpui::test]
fn long_unbreakable_text_wraps_inside_its_card(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, CHAT, 3000.);
    let hash: String = "a3f9c2e17b".repeat(20);
    let display = ToolDisplay::Command { command: format!("git show {hash}"), output: format!("{hash}\n"), exit_code: Some(0) };
    let index = run(&open, cx, "long", display.clone());
    emit(&open, cx, CHAT, vec![AgentEvent::ToolFinished { id: "long".into(), display, is_error: false }]);
    let text = ToolDisplay::Text { title: "fetch".into(), body: format!("https://example.com/{hash}") };
    let text_index = run(&open, cx, "fetch", text);
    let row = bounds(cx, &format!("block-{index}"));
    for (name, line) in [
        (format!("tool-command-{index}"), MONO.line),
        (format!("tool-output-{index}"), MONO.line),
        (format!("tool-text-{text_index}"), LABEL.line),
    ] {
        let row = if name.starts_with("tool-text") { bounds(cx, &format!("block-{text_index}")) } else { row };
        let b = bounds(cx, &name);
        assert!(b.left() >= row.left() && b.right() <= row.right(), "`{name}` {b:?} leaves its row {row:?}");
        // 200 characters do not fit one line of the 760 px column: the text wraps.
        assert!(f32::from(b.size.height) > 2. * line, "`{name}` is {b:?}, it did not wrap");
    }
    // The text ends inside the card: the card's bottom is below the wrapped output.
    let output = bounds(cx, &format!("tool-output-{index}"));
    assert!(output.bottom() <= row.bottom() + px(1.), "{output:?} below {row:?}");
}

#[gpui::test]
fn rows_keep_their_height_while_a_tools_output_grows(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, CHAT, 3000.);
    let index = run(&open, cx, "grow", command("one\ntwo\n"));
    emit(&open, cx, CHAT, vec![AgentEvent::Notice { text: "after the call".into() }]);
    let gap = |cx: &mut VisualTestContext| {
        let (tool, next) = (bounds(cx, &format!("block-{index}")), bounds(cx, &format!("block-{}", index + 1)));
        f32::from(next.top() - tool.bottom())
    };
    let before = bounds(cx, &format!("block-{index}"));
    assert_eq!(gap(cx), 0., "the notice sits right under the tool row");
    let ready = AgentEvent::ToolCallReady {
        id: "grow".into(),
        name: "bash".into(),
        class: ToolClass::Execute,
        display: command("one\ntwo\nthree\nfour\nfive\nsix\n"),
    };
    emit(&open, cx, CHAT, vec![ready]);
    let after = bounds(cx, &format!("block-{index}"));
    assert_eq!(f32::from(after.size.height - before.size.height), 4. * MONO.line, "{before:?} -> {after:?}");
    assert_eq!(gap(cx), 0., "the next row moved down with it");
    let output = bounds(cx, &format!("tool-output-{index}"));
    assert_eq!(f32::from(output.size.height), 6. * MONO.line + 2. * SPACE_1_5 + 1.);
}

#[gpui::test]
fn a_failed_reads_error_wraps_under_its_row_and_can_be_copied(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, CHAT, 3000.);
    let path = format!("src/{}/retry.ts", "deeply-nested-folder".repeat(8));
    let line = |result: &str| ToolDisplay::Line { verb: "read".into(), target: path.clone(), result: result.into() };
    let error = format!("ENOENT: no such file or directory, open '{path}'");
    emit(
        &open,
        cx,
        CHAT,
        vec![
            AgentEvent::TurnStarted,
            AgentEvent::ToolCallStarted { id: "r".into(), name: "read".into() },
            AgentEvent::ToolCallReady { id: "r".into(), name: "read".into(), class: ToolClass::Read, display: line("") },
            AgentEvent::ToolFinished { id: "r".into(), display: line(&error), is_error: true },
        ],
    );
    let index = counts(&open, cx).1 - 1;
    let name = format!("tool-error-{index}");
    let (b, row) = (bounds(cx, &name), bounds(cx, &format!("block-{index}")));
    assert!(b.right() <= row.right() && b.bottom() <= row.bottom() + px(1.), "{b:?} leaves {row:?}");
    assert!(f32::from(b.size.height) > 2. * LABEL.line, "the long error wraps: {b:?}");
    select_first_line(cx, &name, (SPACE_3 + 40. + SPACE_2, 0.), LABEL.line);
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(copy(cx), Some(error));
}
