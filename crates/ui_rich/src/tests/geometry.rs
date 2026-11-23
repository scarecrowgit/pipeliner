//! Geometry through the real element. The test platform's text system is a fake monospace
//! font (13px text: 7.8px per character, 20px lines), which is enough to check soft wrap,
//! vertical movement, scrolling and mouse hits.

use std::rc::Rc;

use gpui::{
    Bounds, Modifiers, MouseButton, MouseDownEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent,
    TestAppContext, TouchPhase, VisualTestContext, point, px,
};

use super::open;
use crate::text::VisualLines;
use crate::text::core::EditorView;
use crate::text::layout::Shaped;
use crate::{SubmitOn, TextArea, TextAreaEvent, TextInput, TextInputEvent};

const CH: f32 = 7.8;
const LH: f32 = 20.;

fn snapshot<V: EditorView>(
    v: &gpui::Entity<V>,
    cx: &mut VisualTestContext,
) -> (Rc<Shaped>, Bounds<Pixels>, Point<Pixels>) {
    v.read_with(cx, |v, _| {
        let c = v.core();
        let (Some(s), Some(b)) = (c.shaped.clone(), c.bounds) else {
            panic!("not painted")
        };
        (s, b, c.scroll)
    })
}

fn area_with<'a>(
    cx: &'a mut TestAppContext,
    min: usize,
    max: usize,
    text: &'static str,
) -> (gpui::Entity<TextArea>, &'a mut VisualTestContext) {
    let (a, _, _, cx) = open::<_, TextAreaEvent>(cx, move |cx| {
        TextArea::new(cx)
            .submit_on(SubmitOn::CmdEnter)
            .rows(min, max)
            .with_text(text)
    });
    cx.run_until_parked();
    (a, cx)
}

const LONG: &str = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi \
omicron pi rho sigma tau upsilon phi chi psi omega alpha beta gamma delta epsilon";

#[gpui::test]
fn long_text_wraps_and_moves_by_visual_lines(cx: &mut TestAppContext) {
    let (a, cx) = area_with(cx, 1, 20, LONG);
    cx.simulate_keystrokes("ctrl-home");
    let (shaped, bounds, _) = snapshot(&a, cx);
    assert_eq!(bounds.size.width, px(400.));
    assert!(shaped.rows() >= 3, "{} rows", shaped.rows());
    assert_eq!(
        bounds.size.height,
        px(LH * shaped.rows() as f32),
        "grows with its content"
    );
    cx.simulate_keystrokes("down");
    let at = a.read_with(cx, |a, _| a.cursor());
    assert_eq!(at, shaped.offset_for_x(1, 0.));
    assert!(
        at > 0 && &LONG[at - 1..at] == " ",
        "line 1 starts after a space: {at}"
    );
    assert_eq!(shaped.caret_position(at), point(px(0.), px(LH)));
    cx.simulate_keystrokes("right right right down");
    let (shaped, _, _) = snapshot(&a, cx);
    let now = a.read_with(cx, |a, _| a.cursor());
    assert_eq!(shaped.line_for_offset(now), 2);
    assert!(
        (shaped.x_for_offset(now) - 3. * CH).abs() < CH,
        "goal column kept"
    );
    cx.simulate_keystrokes("up up");
    a.read_with(cx, |a, _| assert_eq!(a.cursor(), 3));
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(shaped.selection_rects(0..LONG.len()).len(), shaped.rows());
}

#[gpui::test]
fn the_area_grows_to_its_maximum_then_scrolls_to_the_caret(cx: &mut TestAppContext) {
    let (a, cx) = area_with(cx, 2, 3, "");
    let (_, bounds, _) = snapshot(&a, cx);
    assert_eq!(bounds.size.height, px(2. * LH), "minimum rows when empty");
    a.update(cx, |a, cx| a.set_text("1\n2\n3\n4\n5\n6\n7\n8\n9\n10", cx));
    cx.run_until_parked();
    let (_, bounds, scroll) = snapshot(&a, cx);
    assert_eq!(bounds.size.height, px(3. * LH));
    assert_eq!(
        scroll.y,
        px(7. * LH),
        "the caret on the last line is in view"
    );
    cx.simulate_keystrokes("ctrl-home");
    assert_eq!(snapshot(&a, cx).2.y, px(0.));
    let center = bounds.center();
    cx.simulate_mouse_move(center, None, Modifiers::none());
    cx.simulate_event(ScrollWheelEvent {
        position: center,
        delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    assert_eq!(
        snapshot(&a, cx).2.y,
        px(30.),
        "the wheel scrolls the content"
    );
    cx.simulate_keystrokes("down down down down");
    assert_eq!(
        snapshot(&a, cx).2.y,
        px(2. * LH),
        "moving down scrolls just enough"
    );
}

#[gpui::test]
fn the_mouse_places_selects_and_extends(cx: &mut TestAppContext) {
    let (a, cx) = area_with(cx, 1, 5, "hello world\nsecond line");
    let (_, b, _) = snapshot(&a, cx);
    let at = |col: f32, row: f32| b.origin + point(px(col * CH + 1.), px(row * LH + LH / 2.));
    let none = Modifiers::none();
    cx.simulate_mouse_down(at(2., 0.), MouseButton::Left, none);
    a.read_with(cx, |a, _| assert_eq!(a.selection(), 2..2));
    cx.simulate_mouse_move(at(8., 0.), MouseButton::Left, none);
    cx.simulate_mouse_up(at(8., 0.), MouseButton::Left, none);
    a.read_with(cx, |a, _| assert_eq!(a.selection(), 2..8));
    let click = |pos, count| MouseDownEvent {
        button: MouseButton::Left,
        position: pos,
        modifiers: none,
        click_count: count,
        first_mouse: false,
    };
    cx.simulate_event(click(at(7., 0.), 2));
    cx.simulate_mouse_up(at(7., 0.), MouseButton::Left, none);
    a.read_with(cx, |a, _| {
        assert_eq!(a.selection(), 6..11, "double click: the word")
    });
    cx.simulate_event(click(at(3., 1.), 3));
    cx.simulate_mouse_up(at(3., 1.), MouseButton::Left, none);
    a.read_with(cx, |a, _| {
        assert_eq!(a.selection(), 12..23, "triple click: the line")
    });
    cx.simulate_click(at(0., 0.), Modifiers::shift());
    a.read_with(cx, |a, _| {
        assert_eq!(a.selection(), 0..12, "shift-click extends")
    });
    cx.simulate_click(at(40., 0.), none);
    a.read_with(cx, |a, _| {
        assert_eq!(a.selection(), 11..11, "past the line end: the line end")
    });
    cx.simulate_click(at(40., 1.), none);
    a.read_with(cx, |a, _| assert_eq!(a.selection(), 23..23));
}

#[gpui::test]
fn a_single_line_scrolls_sideways_to_the_caret(cx: &mut TestAppContext) {
    let (i, _, _, cx) = open::<_, TextInputEvent>(cx, |cx| TextInput::new(cx).with_text(LONG));
    cx.run_until_parked();
    let (shaped, b, scroll) = snapshot(&i, cx);
    assert_eq!(shaped.rows(), 1);
    assert_eq!(b.size.height, px(LH));
    let overflow = f32::from(shaped.width()) + 1. - f32::from(b.size.width);
    assert!(
        overflow > 0. && (f32::from(scroll.x) - overflow).abs() < 0.5,
        "{scroll:?}"
    );
    cx.simulate_keystrokes("home");
    assert_eq!(snapshot(&i, cx).2.x, px(0.));
}
