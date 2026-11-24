//! A line longer than the card: it scrolls sideways inside the card, the line numbers stay
//! where they are, and selection and copy still work on the scrolled code.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, ElementId, IntoElement, Modifiers, MouseButton, Pixels, Point, Render, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, Window, div, point,
    prelude::*, px, ScrollHandle,
};

use super::super::select::{Shared, row_point, shared_of};
use super::*;
use crate::markdown::bind_keys;

const CARD_W: f32 = 320.;

fn long_line() -> String {
    format!("let message = \"{}\";", "retry the charge once the gateway answers ".repeat(6))
}

struct Host {
    file: Arc<DiffFile>,
    page: ScrollHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The card sits in a page that scrolls down, as in the Inbox and the chat.
        ui::root().child(
            div()
                .id("page")
                .flex()
                .flex_col()
                .h(px(400.))
                .overflow_y_scroll()
                .track_scroll(&self.page)
                .child(
                    div()
                        .w(px(CARD_W))
                        .flex_none()
                        .debug_selector(|| "card-box".into())
                        .child(DiffCard::new("long", self.file.clone()).compact()),
                )
                .child(div().flex_none().h(px(2000.))),
        )
    }
}

fn open(cx: &mut TestAppContext) -> (ScrollHandle, &mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        bind_keys(cx);
    });
    let file = DiffFile {
        path: "src/charge.rs".into(),
        added: 1,
        hunks: vec![DiffHunk { lines: vec![ctx(1, 1, "fn charge() {"), add(2, &long_line()), ctx(2, 3, "}")] }],
        ..Default::default()
    };
    let page = ScrollHandle::new();
    let handle = page.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Host { file: Arc::new(file.clone()), page: handle.clone() });
    cx.run_until_parked();
    (page, cx)
}

fn shared(cx: &mut VisualTestContext) -> Rc<RefCell<Shared>> {
    cx.run_until_parked();
    shared_of(&ElementId::NamedChild(Box::new("long".into()), "view".into())).unwrap_or_else(|| panic!("diff not shown"))
}

fn at(cx: &mut VisualTestContext, row: usize, offset: usize) -> Point<Pixels> {
    let s = shared(cx);
    let p = row_point(&s.borrow(), row, offset);
    p.unwrap_or_else(|| panic!("row {row} not laid out"))
}

fn wheel(cx: &mut VisualTestContext, at: Point<Pixels>, delta: Point<Pixels>, modifiers: Modifiers) {
    cx.simulate_event(ScrollWheelEvent { position: at, delta: ScrollDelta::Pixels(delta), modifiers, touch_phase: TouchPhase::Moved });
    cx.run_until_parked();
}

#[gpui::test]
fn a_long_line_scrolls_sideways_under_a_gutter_that_stays(cx: &mut TestAppContext) {
    let (page, cx) = open(cx);
    let Some(card) = cx.debug_bounds("card-box") else { panic!("no card") };
    let end = long_line().len();
    let (start, tail) = (at(cx, 1, 0), at(cx, 1, end));
    assert!(tail.x > card.right(), "the line is longer than the card: {tail:?}");
    let inside = point(card.center().x, start.y);

    // A sideways wheel (a touchpad; Shift and a mouse wheel, which the platform turns
    // sideways) moves the code.
    wheel(cx, inside, point(px(-200.), px(0.)), Modifiers::none());
    let moved = at(cx, 1, 0);
    assert!(moved.x < start.x - px(150.), "the code scrolled left: {start:?} → {moved:?}");
    assert_eq!(page.offset().y, px(0.), "the page around it did not take the sideways wheel as a vertical one");
    assert!((moved.y - start.y).abs() < px(0.5));
    // Far enough, the end of the line shows inside the card.
    for _ in 0..10 {
        wheel(cx, inside, point(px(-400.), px(0.)), Modifiers::none());
    }
    let tail = at(cx, 1, end);
    assert!(tail.x <= card.right() && tail.x > card.left() + px(60.), "the end of the line is in view: {tail:?} in {card:?}");

    // The code stays selectable where it now is: drag over its tail and copy.
    let from = at(cx, 1, end - "answers \";".len());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(tail, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(tail, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("answers \";"));
}
