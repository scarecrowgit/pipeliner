//! Editors as tab stops: Tab and Shift-Tab move between fields in paint order, a text area
//! can type tabs instead, and a press on a field's frame below the text focuses it even
//! with the overlay host behind it.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AppContext as _, Context, Entity, Modifiers, Render, TestAppContext, VisualTestContext, Window,
    div, point, prelude::*, px,
};
use ui::OverlayHost;

use crate::{TextArea, TextInput, text_area_field, text_field};

/// Two form inputs, a text area and a literal-tab area, in that order, in frames.
struct Form {
    first: Entity<TextInput>,
    second: Entity<TextInput>,
    notes: Entity<TextArea>,
    code: Entity<TextArea>,
}

impl Render for Form {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(400.))
            .flex()
            .flex_col()
            .gap_2()
            .child(text_field("first", &self.first))
            .child(text_field("second", &self.second).mono().danger(true))
            .child(text_area_field("notes", &self.notes))
            .child(text_area_field("code", &self.code).mono())
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Form>, VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        crate::bind_keys(cx);
    });
    let slot: Rc<RefCell<Option<Entity<Form>>>> = Rc::default();
    let s = slot.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let form = cx.new(|cx| Form {
            first: cx.new(|cx| TextInput::new(cx).tab_moves_focus(true)),
            second: cx.new(|cx| TextInput::new(cx).tab_moves_focus(true)),
            notes: cx.new(|cx| TextArea::new(cx).rows(1, 4)),
            code: cx.new(|cx| TextArea::new(cx).rows(1, 4).literal_tabs(true)),
        });
        *s.borrow_mut() = Some(form.clone());
        OverlayHost::new(window, cx).with_content(form)
    });
    let mut vcx = vcx.clone();
    let Some(form) = slot.borrow_mut().take() else { panic!("no form") };
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    (form, vcx)
}

/// Which field has focus: 0 first, 1 second, 2 notes, 3 code.
fn focused(form: &Entity<Form>, cx: &mut VisualTestContext) -> Option<usize> {
    cx.update(|window, cx| {
        let f = form.read(cx);
        [
            f.first.read(cx).is_focused(window),
            f.second.read(cx).is_focused(window),
            f.notes.read(cx).is_focused(window),
            f.code.read(cx).is_focused(window),
        ]
        .iter()
        .position(|on| *on)
    })
}

#[gpui::test]
fn tab_and_shift_tab_move_between_fields(cx: &mut TestAppContext) {
    let (form, mut cx) = open(cx);
    cx.update(|window, cx| form.read(cx).first.read(cx).focus(window));
    cx.run_until_parked();
    assert_eq!(focused(&form, &mut cx), Some(0));
    cx.simulate_keystrokes("tab");
    assert_eq!(focused(&form, &mut cx), Some(1));
    cx.simulate_keystrokes("tab");
    assert_eq!(focused(&form, &mut cx), Some(2), "into the text area");
    cx.simulate_keystrokes("tab");
    assert_eq!(focused(&form, &mut cx), Some(3), "a text area passes Tab on");
    cx.simulate_keystrokes("shift-tab shift-tab");
    assert_eq!(focused(&form, &mut cx), Some(1));
    form.read_with(&cx, |f, cx| assert!(f.notes.read(cx).text().is_empty() && f.second.read(cx).text().is_empty()));
}

#[gpui::test]
fn a_literal_tab_area_types_tabs_and_shift_tab_still_leaves(cx: &mut TestAppContext) {
    let (form, mut cx) = open(cx);
    cx.update(|window, cx| form.read(cx).code.read(cx).focus(window));
    cx.simulate_input("fn");
    cx.simulate_keystrokes("tab");
    cx.simulate_input("x");
    form.read_with(&cx, |f, cx| assert_eq!(f.code.read(cx).text(), "fn\tx"));
    assert_eq!(focused(&form, &mut cx), Some(3));
    cx.simulate_keystrokes("shift-tab");
    assert_eq!(focused(&form, &mut cx), Some(2));
}

#[gpui::test]
fn a_press_under_the_text_of_a_field_focuses_it(cx: &mut TestAppContext) {
    let (form, mut cx) = open(cx);
    let frame = cx.debug_bounds("notes").expect("the notes frame");
    // The frame is at least 72px; one row of text leaves room under it.
    let below = point(frame.center().x, frame.bottom() - px(6.));
    cx.simulate_click(below, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(focused(&form, &mut cx), Some(2), "the host did not take focus back");
    let first = cx.debug_bounds("first").expect("the first frame");
    cx.simulate_click(point(first.right() - px(4.), first.center().y), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(focused(&form, &mut cx), Some(0));
}
