use gpui::{ClipboardItem, Modifiers, Render, TestAppContext, prelude::*};

use super::{clear, open};
use crate::{TextInput, TextInputEvent as Ev, text_field};

fn input(cx: &mut gpui::Context<TextInput>) -> TextInput {
    TextInput::new(cx).placeholder("Search")
}

#[gpui::test]
fn pasted_newlines_become_spaces(cx: &mut TestAppContext) {
    let (i, log, _, cx) = open(cx, input);
    cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo\r\nthree".into()));
    cx.simulate_keystrokes("ctrl-v");
    i.read_with(cx, |i, _| assert_eq!(i.text(), "one two three"));
    assert!(log.borrow().contains(&Ev::Changed));
    i.update(cx, |i, cx| i.set_text("a\nb", cx));
    i.read_with(cx, |i, _| assert_eq!(i.text(), "a b"));
}

#[gpui::test]
fn enter_arrows_and_tab_are_emitted(cx: &mut TestAppContext) {
    let (i, log, _, cx) = open(cx, input);
    cx.simulate_input("ab");
    clear(&log);
    cx.simulate_keystrokes("up down tab enter shift-enter escape");
    assert_eq!(
        log.borrow().as_slice(),
        &[Ev::Up, Ev::Down, Ev::Tab, Ev::Submit, Ev::Cancel]
    );
    i.read_with(cx, |i, _| assert_eq!((i.text(), i.cursor()), ("ab", 2)));
}

#[gpui::test]
fn word_keys_and_home_end(cx: &mut TestAppContext) {
    let (i, _, _, cx) = open(cx, input);
    cx.simulate_input("foo");
    cx.simulate_keystrokes("space");
    cx.simulate_input("bar");
    cx.simulate_keystrokes("ctrl-left");
    i.read_with(cx, |i, _| assert_eq!(i.cursor(), 4));
    cx.simulate_keystrokes("home shift-end");
    i.read_with(cx, |i, _| assert_eq!(i.selection(), 0..7));
    cx.simulate_keystrokes("right ctrl-backspace");
    i.read_with(cx, |i, _| assert_eq!(i.text(), "foo "));
}

#[gpui::test]
fn a_protected_suffix_survives_editing(cx: &mut TestAppContext) {
    let (i, _, _, cx) = open(cx, input);
    i.update(cx, |i, cx| {
        i.set_text("notes.md", cx);
        i.set_protected_suffix(3, cx);
        i.select_all(cx);
    });
    cx.simulate_input("plan");
    cx.simulate_keystrokes("end delete");
    i.read_with(cx, |i, _| assert_eq!(i.text(), "plan.md"));
}

struct Field(gpui::Entity<TextInput>);

impl Render for Field {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        ui::root().child(
            gpui::div()
                .w(gpui::px(300.))
                .child(text_field("search", &self.0)),
        )
    }
}

#[gpui::test]
fn a_click_on_the_field_focuses_the_input(cx: &mut TestAppContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        crate::bind_keys(cx);
    });
    let (field, cx) = cx.add_window_view(|_, cx| Field(cx.new(input)));
    let input = field.read_with(cx, |f, _| f.0.clone());
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let Some(bounds) = cx.debug_bounds("search") else {
        panic!("field not laid out")
    };
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    cx.simulate_input("ok");
    input.read_with(cx, |i, _| assert_eq!(i.text(), "ok"));
    // A click inside the text places the caret.
    cx.simulate_click(
        bounds.origin + gpui::point(gpui::px(13.), gpui::px(20.)),
        Modifiers::none(),
    );
    cx.run_until_parked();
    input.read_with(cx, |i, _| assert!(i.cursor() <= 2));
}
