//! A selection group in a test window: log-like lines, each its own verbatim view, with one
//! selection across them, driven through the window's mouse and key dispatch.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Context, Entity, Focusable, IntoElement, Modifiers, MouseButton, Pixels, Point, Render, TestAppContext,
    VisualTestContext, Window, div, point, prelude::*, px,
};
use ui::layout::col;
use ui::theme::LABEL;

use crate::markdown::element::unit_point;
use crate::markdown::{Markdown, SelectionGroup, bind_keys};

const LINES: [&str; 7] = [
    "worktree ready",
    "read src/refunds.ts",
    "bash cargo test",
    "3 tests failed",
    "edit src/refunds.ts",
    "bash cargo test",
    "all tests passed",
];

/// The lines as views in one group (keyed 10, 20, … like log line numbers), a few hidden
/// as a scrolled virtual list would have them, then a line outside the group.
struct Host {
    /// A row's view, `None` while its row is scrolled out (the view is dropped).
    lines: Rows,
    outside: Entity<Markdown>,
}

type Rows = Rc<RefCell<Vec<Option<Entity<Markdown>>>>>;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let lines = self.lines.borrow().clone();
        ui::root().child(
            col()
                .w(px(400.))
                .children(lines.into_iter().flatten().map(|v| div().w(px(400.)).child(v)))
                .child(div().h(px(40.)))
                .child(div().w(px(400.)).child(self.outside.clone()))
                .child(div().h(px(200.)).w_full().debug_selector(|| "blank".into())),
        )
    }
}

struct World {
    group: SelectionGroup,
    lines: Vec<Entity<Markdown>>,
    rows: Rows,
    outside: Entity<Markdown>,
}

fn key(ix: usize) -> usize {
    (ix + 1) * 10
}

fn open(cx: &mut TestAppContext) -> (World, &mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        bind_keys(cx);
    });
    let group = SelectionGroup::new();
    group.set_texts(LINES.iter().enumerate().map(|(ix, t)| (key(ix), (*t).into())));
    let slot: Rc<RefCell<Option<World>>> = Rc::default();
    let (s, g) = (slot.clone(), group.clone());
    let (_, cx) = cx.add_window_view(move |_, cx| {
        let lines: Vec<Entity<Markdown>> = LINES
            .iter()
            .enumerate()
            .map(|(ix, t)| cx.new(|cx| Markdown::verbatim(*t, cx).type_style(LABEL).group(&g, key(ix))))
            .collect();
        let outside = cx.new(|cx| Markdown::verbatim("a line of its own", cx).type_style(LABEL));
        let rows: Rows = Rc::new(RefCell::new(lines.iter().cloned().map(Some).collect()));
        *s.borrow_mut() = Some(World { group: g.clone(), lines, rows: rows.clone(), outside: outside.clone() });
        Host { lines: rows, outside }
    });
    cx.run_until_parked();
    let Some(world) = slot.borrow_mut().take() else { panic!("no world") };
    (world, cx)
}

fn at(md: &Entity<Markdown>, cx: &mut VisualTestContext, offset: usize) -> Point<Pixels> {
    md.read_with(cx, |m, _| unit_point(&m.shared.borrow(), 0, offset)).unwrap_or_else(|| panic!("offset {offset} not laid out"))
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    // Through the lines in between, as a real drag goes.
    let mid = point(from.x, (from.y + to.y) / 2.);
    cx.simulate_mouse_move(mid, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

fn own(md: &Entity<Markdown>, cx: &mut VisualTestContext) -> Option<String> {
    md.read_with(cx, |m, _| m.selected_text())
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

#[gpui::test]
fn a_drag_across_lines_selects_through_them_and_copies_them_joined(cx: &mut TestAppContext) {
    let (w, cx) = open(cx);
    let from = at(&w.lines[1], cx, "read ".len());
    let to = at(&w.lines[5], cx, "bash".len());
    drag(cx, from, to);
    // Every line shows its part.
    assert_eq!(own(&w.lines[0], cx), None);
    assert_eq!(own(&w.lines[1], cx).as_deref(), Some("src/refunds.ts"));
    assert_eq!(own(&w.lines[3], cx).as_deref(), Some("3 tests failed"), "a line in between is whole");
    assert_eq!(own(&w.lines[5], cx).as_deref(), Some("bash"));
    assert_eq!(own(&w.lines[6], cx), None);
    cx.simulate_keystrokes("ctrl-c");
    let five = "src/refunds.ts\nbash cargo test\n3 tests failed\nedit src/refunds.ts\nbash";
    assert_eq!(clipboard(cx).as_deref(), Some(five));

    // Upwards, from the middle of a line to the start of an earlier one.
    let (from, to) = (at(&w.lines[4], cx, "edit".len()), at(&w.lines[2], cx, 0));
    drag(cx, from, to);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("bash cargo test\n3 tests failed\nedit"));

    // A press on nothing clears every line.
    let Some(blank) = cx.debug_bounds("blank") else { panic!("no blank") };
    cx.simulate_click(blank.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(w.group.is_empty());
    assert!(w.lines.iter().all(|l| own(l, cx).is_none()));
}

#[gpui::test]
fn a_drag_past_the_last_line_selects_to_its_end_and_a_line_outside_stays_alone(cx: &mut TestAppContext) {
    let (w, cx) = open(cx);
    // Below the last line, in the gap: to the end of the last line.
    let from = at(&w.lines[5], cx, "bash ".len());
    let below = at(&w.lines[6], cx, 0) + point(px(0.), px(25.));
    drag(cx, from, below);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("cargo test\nall tests passed"));
    // A drag in the line outside the group is its own and clears the group.
    let (a, b) = (at(&w.outside, cx, 2), at(&w.outside, cx, "a line".len()));
    drag(cx, a, b);
    assert!(w.group.is_empty());
    assert_eq!(own(&w.outside, cx).as_deref(), Some("line"));
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("line"));
}

#[gpui::test]
fn lines_without_a_view_are_copied_from_the_texts_and_get_their_part_back(cx: &mut TestAppContext) {
    let (w, cx) = open(cx);
    let World { group, lines, rows, .. } = w;
    let from = at(&lines[0], cx, "work".len());
    let to = at(&lines[6], cx, "all".len());
    drag(cx, from, to);
    // Lines 2–4 scroll out of a virtual list: their views are dropped.
    let first = lines[0].clone();
    let weak: Vec<_> = lines[2..5].iter().map(|v| v.downgrade()).collect();
    drop(lines);
    for row in &mut rows.borrow_mut()[2..5] {
        *row = None;
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(weak.iter().all(|v| v.upgrade().is_none()), "the views are gone");
    let all = "tree ready\nread src/refunds.ts\nbash cargo test\n3 tests failed\nedit src/refunds.ts\nbash cargo test\nall";
    // Copied from a line that is still there, the lines without a view come from the texts.
    cx.update(|window, cx| window.focus(&first.read(cx).focus_handle(cx)));
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some(all));
    // Line 3 scrolls back in with a new view: it shows its part at once.
    let back = cx.update(|_, cx| cx.new(|cx| Markdown::verbatim(LINES[3], cx).type_style(LABEL).group(&group, key(3))));
    rows.borrow_mut()[3] = Some(back.clone());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(own(&back, cx).as_deref(), Some("3 tests failed"));
    assert_eq!(group.selected_text().as_deref(), Some(all));
}
