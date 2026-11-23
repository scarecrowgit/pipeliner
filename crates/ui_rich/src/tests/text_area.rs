use gpui::{ClipboardItem, EntityInputHandler, Image, ImageFormat, TestAppContext};

use super::{clear, open};
use crate::{SubmitOn, TextArea, TextAreaEvent as Ev};

fn area(mode: SubmitOn) -> impl FnOnce(&mut gpui::Context<TextArea>) -> TextArea {
    move |cx| {
        TextArea::new(cx)
            .submit_on(mode)
            .placeholder("Write a comment")
    }
}

#[gpui::test]
fn typed_input_reaches_the_buffer(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::CmdEnter));
    assert_eq!(log.borrow().as_slice(), &[Ev::Focus]);
    clear(&log);
    cx.simulate_input("héllo");
    cx.simulate_keystrokes("space");
    cx.simulate_input("мир");
    a.read_with(cx, |a, _| {
        assert_eq!(a.text(), "héllo мир");
        assert_eq!(a.cursor(), a.text().len());
    });
    assert_eq!(log.borrow().len(), 9);
    assert!(log.borrow().iter().all(|e| *e == Ev::Changed));
}

#[gpui::test]
fn enter_submits_in_enter_mode(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::Enter));
    cx.simulate_input("hi");
    clear(&log);
    cx.simulate_keystrokes("enter");
    assert_eq!(log.borrow().as_slice(), &[Ev::Submit]);
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("x");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "hi\nx"));
    clear(&log);
    cx.simulate_keystrokes("ctrl-enter");
    assert_eq!(log.borrow().as_slice(), &[Ev::Submit]);
}

#[gpui::test]
fn cmd_enter_submits_in_cmd_enter_mode(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::CmdEnter));
    cx.simulate_input("a");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("b");
    cx.simulate_keystrokes("shift-enter");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "a\nb\n"));
    assert!(!log.borrow().contains(&Ev::Submit));
    cx.simulate_keystrokes("ctrl-enter");
    cx.simulate_keystrokes("cmd-enter");
    assert_eq!(log.borrow().iter().filter(|e| **e == Ev::Submit).count(), 2);
}

#[gpui::test]
fn never_mode_does_not_submit(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::Never));
    cx.simulate_keystrokes("enter ctrl-enter");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "\n"));
    assert!(!log.borrow().contains(&Ev::Submit));
}

#[gpui::test]
fn intercepted_arrows_are_emitted_instead_of_moving(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::CmdEnter));
    cx.simulate_input("ab");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("cd");
    a.update(cx, |a, _| a.intercept_arrows(true));
    clear(&log);
    cx.simulate_keystrokes("up down tab enter");
    assert_eq!(
        log.borrow().as_slice(),
        &[Ev::Up, Ev::Down, Ev::Tab, Ev::Confirm]
    );
    a.read_with(cx, |a, _| assert_eq!((a.text(), a.cursor()), ("ab\ncd", 5)));
    a.update(cx, |a, _| a.intercept_arrows(false));
    clear(&log);
    cx.simulate_keystrokes("up");
    a.read_with(cx, |a, _| assert_eq!(a.cursor(), 2));
    cx.simulate_keystrokes("tab");
    assert!(
        !log.borrow().contains(&Ev::Tab),
        "tab is not emitted without interception"
    );
}

#[gpui::test]
fn tokens_are_detected_and_replaced(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::Enter));
    cx.simulate_input("ask");
    cx.simulate_keystrokes("space");
    cx.simulate_input("@re");
    let token = a.read_with(cx, |a, _| a.token_before_cursor(&['@', '/']));
    let Some(token) = token else {
        panic!("no token")
    };
    assert_eq!(
        (token.trigger, token.start, token.text.as_str()),
        ('@', 4, "re")
    );
    clear(&log);
    a.update(cx, |a, cx| a.replace_token(&token, "@reviewer ", cx));
    a.read_with(cx, |a, _| {
        assert_eq!(a.text(), "ask @reviewer ");
        assert_eq!(a.token_before_cursor(&['@']), None);
    });
    assert_eq!(log.borrow().as_slice(), &[Ev::Changed]);
    a.update(cx, |a, cx| a.set_text("me@host", cx));
    a.read_with(cx, |a, _| assert_eq!(a.token_before_cursor(&['@']), None));
}

#[gpui::test]
fn set_text_does_not_emit_but_insert_does(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::Enter));
    clear(&log);
    a.update(cx, |a, cx| a.set_text("draft", cx));
    assert!(log.borrow().is_empty());
    a.update(cx, |a, cx| a.insert_text("!", cx));
    assert_eq!(log.borrow().as_slice(), &[Ev::Changed]);
    a.read_with(cx, |a, _| assert_eq!(a.text(), "draft!"));
}

#[gpui::test]
fn editing_keys_undo_and_redo(cx: &mut TestAppContext) {
    let (a, _, _, cx) = open(cx, area(SubmitOn::Enter));
    cx.simulate_input("one");
    cx.simulate_keystrokes("space");
    cx.simulate_input("two");
    cx.simulate_keystrokes("alt-backspace");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "one "));
    cx.simulate_keystrokes("ctrl-z");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "one two"));
    cx.simulate_keystrokes("ctrl-z");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "one"));
    cx.simulate_keystrokes("ctrl-shift-z cmd-shift-z");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "one "));
    cx.simulate_keystrokes("home");
    cx.simulate_keystrokes("shift-right shift-right");
    a.read_with(cx, |a, _| assert_eq!(a.selection(), 0..2));
    cx.simulate_keystrokes("backspace");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "e "));
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("z");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "z"));
    cx.simulate_keystrokes("backspace backspace");
    a.read_with(cx, |a, _| assert!(a.is_empty()));
}

#[gpui::test]
fn clipboard_copy_cut_and_paste(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::Enter));
    cx.simulate_input("abc");
    cx.simulate_keystrokes("cmd-a ctrl-c end");
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("abc")
    );
    cx.simulate_keystrokes("ctrl-v");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "abcabc"));
    cx.write_to_clipboard(ClipboardItem::new_string("x\r\ny".into()));
    cx.simulate_keystrokes("cmd-v");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "abcabcx\ny"));
    cx.simulate_keystrokes("ctrl-a ctrl-x");
    a.read_with(cx, |a, _| assert!(a.is_empty()));
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("abcabcx\ny")
    );
    clear(&log);
    let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);
    cx.write_to_clipboard(ClipboardItem::new_image(&image));
    cx.simulate_keystrokes("ctrl-v");
    let got = log.borrow().clone();
    assert!(
        matches!(got.as_slice(), [Ev::PasteImage(item)] if item.text().is_none()),
        "{got:?}"
    );
    a.read_with(cx, |a, _| assert!(a.is_empty()));
}

#[gpui::test]
fn escape_cancels(cx: &mut TestAppContext) {
    let (_, log, _, cx) = open(cx, area(SubmitOn::Enter));
    clear(&log);
    cx.simulate_keystrokes("escape");
    assert_eq!(log.borrow().as_slice(), &[Ev::Cancel]);
}

#[gpui::test]
fn focus_and_blur_are_reported(cx: &mut TestAppContext) {
    let (a, log, other, cx) = open(cx, area(SubmitOn::Enter));
    assert_eq!(log.borrow().as_slice(), &[Ev::Focus]);
    cx.update(|window, _| other.focus(window));
    cx.run_until_parked();
    assert_eq!(log.borrow().as_slice(), &[Ev::Focus, Ev::Blur]);
    cx.simulate_input("x");
    a.read_with(cx, |a, _| {
        assert!(a.is_empty(), "typing goes nowhere without focus")
    });
    a.update_in(cx, |a, window, _| a.focus(window));
    cx.run_until_parked();
    assert_eq!(log.borrow().last(), Some(&Ev::Focus));
}

#[gpui::test]
fn ime_composition_goes_through_the_input_handler(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::Enter));
    cx.simulate_input("a");
    a.update_in(cx, |a, window, cx| {
        a.replace_and_mark_text_in_range(None, "k", None, window, cx);
        a.replace_and_mark_text_in_range(None, "か", Some(1..1), window, cx);
        assert_eq!(a.marked_text_range(window, cx), Some(1..2));
        assert_eq!(a.text(), "aか");
        a.replace_text_in_range(None, "火", window, cx);
        assert_eq!(a.marked_text_range(window, cx), None);
        // UTF-16 ranges from the platform: 😀 is two units.
        a.replace_text_in_range(Some(2..2), "😀", window, cx);
        assert_eq!(a.text(), "a火😀");
        let sel = a.selected_text_range(false, window, cx).map(|s| s.range);
        assert_eq!(sel, Some(4..4));
        let mut adjusted = None;
        assert_eq!(
            a.text_for_range(1..4, &mut adjusted, window, cx).as_deref(),
            Some("火😀")
        );
        assert_eq!(adjusted, Some(1..4));
    });
    assert!(log.borrow().iter().filter(|e| **e == Ev::Changed).count() >= 4);
    cx.simulate_keystrokes("ctrl-z");
    a.read_with(cx, |a, _| assert_eq!(a.text(), "a火"));
}

#[gpui::test]
fn a_disabled_area_ignores_input(cx: &mut TestAppContext) {
    let (a, log, _, cx) = open(cx, area(SubmitOn::Enter));
    a.update(cx, |a, cx| a.set_disabled(true, cx));
    cx.run_until_parked();
    clear(&log);
    cx.simulate_input("x");
    cx.simulate_keystrokes("ctrl-v backspace");
    a.read_with(cx, |a, _| assert!(a.is_empty()));
    assert!(!log.borrow().contains(&Ev::Changed));
}

#[gpui::test]
fn caret_bounds_are_known_after_paint(cx: &mut TestAppContext) {
    let (a, _, _, cx) = open(cx, area(SubmitOn::Enter));
    cx.simulate_input("hello");
    cx.run_until_parked();
    let b = a.read_with(cx, |a, _| a.caret_bounds());
    let Some(b) = b else {
        panic!("no caret bounds after paint")
    };
    assert!(b.size.height > gpui::px(0.));
}
