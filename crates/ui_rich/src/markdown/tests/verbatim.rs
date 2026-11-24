//! Verbatim mode: the text model (tabs shown as spaces, copied as tabs) and the view in a
//! test window: what is laid out, selection and copy, the type style, and the size it gets
//! in a row next to a fixed sibling.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, Context, Entity, IntoElement, Modifiers, MouseButton, Render,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};
use ui::layout::{col, row};
use ui::theme::{BODY, DANGER, FONT_MONO, FONT_SANS, LABEL, MONO_SM};

use crate::markdown::element::unit_point;
use crate::markdown::rich::RichText;
use crate::markdown::{Markdown, bind_keys};

/// Every Markdown marker a tool output may contain, none of which may be interpreted.
const MARKUP: &str = "# x\n*a* and _c_\n`b`\n- item\n\n    indented\n1. one\n\n> quote";

#[test]
fn tabs_show_as_spaces_and_copy_as_tabs() {
    let rt = RichText::verbatim("a\tb\t\tc");
    assert_eq!(rt.text, "a    b        c");
    assert_eq!(rt.tabs, [1, 6, 10]);
    assert_eq!(rt.source(0..rt.text.len()), "a\tb\t\tc");
    // A range touching part of a shown tab copies the tab.
    assert_eq!(rt.source(2..5), "\t");
    assert_eq!(rt.source(2..6), "\tb");
    assert_eq!(rt.source(0..2), "a\t");
    assert_eq!(rt.source(5..7), "b\t");
    assert_eq!(rt.source(3..3), "");
    let mut grown = RichText::verbatim("a\tb");
    grown.extend_verbatim("\t\tc");
    assert_eq!(grown, rt);
    assert_eq!(grown.spans.len(), 1);
    assert_eq!(grown.spans[0].range, 0..grown.text.len());
}

/// How the test window places the views.
#[derive(Clone, Copy)]
enum Place {
    /// One under the other in a 640px column.
    Stack,
    /// A 400px row: a fixed 100px sibling, then the view in a `flex_1().min_w_0()` column
    /// (or a plain div when `col` is false), then a row below.
    Row { col: bool },
}

struct Host {
    views: Vec<AnyView>,
    place: Place,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let views = self.views.iter().cloned();
        let body: AnyElement = match self.place {
            Place::Stack => col()
                .w(px(640.))
                .gap(px(24.))
                .children(
                    views
                        .enumerate()
                        .map(|(i, v)| div().debug_selector(move || format!("view-{i}")).child(v)),
                )
                .into_any_element(),
            Place::Row { col: as_col } => {
                let text = if as_col { col() } else { div() };
                col()
                    .w(px(400.))
                    .child(
                        row()
                            .w(px(400.))
                            .items_start()
                            .child(div().flex_none().w(px(100.)).h(px(18.)))
                            .child(
                                text.flex_1()
                                    .min_w_0()
                                    .debug_selector(|| "text-col".into())
                                    .children(views),
                            ),
                    )
                    .child(div().h(px(10.)).w_full().debug_selector(|| "below".into()))
                    .into_any_element()
            }
        };
        ui::root().child(body)
    }
}

/// A window showing the views `make` builds, placed as `place` says.
fn open(
    cx: &mut TestAppContext,
    place: Place,
    make: impl Fn(&mut Window, &mut Context<Host>) -> Vec<Entity<Markdown>> + 'static,
) -> (Vec<Entity<Markdown>>, &mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        bind_keys(cx);
    });
    let slot: Rc<RefCell<Vec<Entity<Markdown>>>> = Rc::default();
    let s = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let mds = make(window, cx);
        *s.borrow_mut() = mds.clone();
        Host {
            views: mds.into_iter().map(Into::into).collect(),
            place,
        }
    });
    cx.run_until_parked();
    let mds = slot.borrow().clone();
    (mds, cx)
}

fn one(
    cx: &mut TestAppContext,
    place: Place,
    make: impl Fn(&mut Context<Markdown>) -> Markdown + 'static,
) -> (Entity<Markdown>, &mut VisualTestContext) {
    let (mds, cx) = open(cx, place, move |_, cx| vec![cx.new(&make)]);
    (mds[0].clone(), cx)
}

fn point_of(
    md: &Entity<Markdown>,
    cx: &mut VisualTestContext,
    offset: usize,
) -> gpui::Point<gpui::Pixels> {
    md.read_with(cx, |m, _| unit_point(&m.shared.borrow(), 0, offset))
        .unwrap_or_else(|| panic!("offset {offset} not laid out"))
}

fn drag(
    cx: &mut VisualTestContext,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

#[gpui::test]
fn markup_is_laid_out_as_written(cx: &mut TestAppContext) {
    let (md, cx) = one(cx, Place::Stack, |cx| Markdown::verbatim(MARKUP, cx));
    md.read_with(cx, |m, _| {
        assert!(m.is_verbatim());
        assert!(m.document().blocks.is_empty());
        let s = m.shared.borrow();
        assert_eq!(s.frame.len(), 1);
        let layout = &s.frame[0].layout;
        assert_eq!(layout.rich.text, MARKUP);
        assert_eq!(layout.rich.spans.len(), 1);
        assert!(layout.rich.links.is_empty());
        // Every newline is a line break, blank lines and the indent included.
        assert_eq!(layout.line_texts(), MARKUP.split('\n').collect::<Vec<_>>());
    });
}

#[gpui::test]
fn drag_across_two_lines_copies_the_substring(cx: &mut TestAppContext) {
    let text = "first line here\nsecond line there";
    let (md, cx) = one(cx, Place::Stack, move |cx| Markdown::verbatim(text, cx));
    let from = point_of(&md, cx, "first ".len());
    let to = point_of(&md, cx, "first line here\nsecond".len());
    drag(cx, from, to);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("line here\nsecond"));
}

#[gpui::test]
fn select_all_copies_the_source(cx: &mut TestAppContext) {
    let text = "$ make test\n\tcc -o x x.c\n  *warning*: `unused`\n\n# done\n";
    let (md, cx) = one(cx, Place::Stack, move |cx| Markdown::verbatim(text, cx));
    let at = point_of(&md, cx, 2);
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some(text));
}

#[gpui::test]
fn streaming_output_grows_and_keeps_the_selection(cx: &mut TestAppContext) {
    let (md, cx) = one(cx, Place::Stack, |cx| {
        Markdown::verbatim("line one\n", cx).type_style(MONO_SM)
    });
    let from = point_of(&md, cx, 0);
    let to = point_of(&md, cx, "line".len());
    drag(cx, from, to);
    md.update(cx, |m, cx| m.set_text("line one\n\tline two\n# three", cx));
    cx.run_until_parked();
    md.read_with(cx, |m, _| {
        assert_eq!(m.selected_text().as_deref(), Some("line"));
        let s = m.shared.borrow();
        assert_eq!(s.frame.len(), 1);
        let layout = &s.frame[0].layout;
        assert_eq!(layout.line_texts(), ["line one", "    line two", "# three"]);
    });
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(
        md.read_with(cx, |m, _| m.selected_text()).as_deref(),
        Some("line one\n\tline two\n# three")
    );
    // A text that is not a continuation replaces the old one.
    md.update(cx, |m, cx| m.set_text("other", cx));
    cx.run_until_parked();
    md.read_with(cx, |m, _| {
        assert_eq!(m.shared.borrow().frame[0].layout.rich.text, "other");
    });
}

#[gpui::test]
fn a_mono_style_shapes_in_the_mono_font(cx: &mut TestAppContext) {
    let text = "Compiling ui_rich\n    Finished `dev` profile";
    let (mds, cx) = open(cx, Place::Stack, move |_, cx| {
        vec![
            cx.new(|cx| Markdown::verbatim(text, cx).type_style(MONO_SM)),
            cx.new(|cx| Markdown::verbatim(text, cx).type_style(LABEL).color(DANGER)),
        ]
    });
    let fonts = |md: &Entity<Markdown>, cx: &mut VisualTestContext| {
        md.read_with(cx, |m, _| m.shared.borrow().frame[0].layout.fonts())
    };
    let mono = fonts(&mds[0], cx);
    assert!(!mono.is_empty());
    for (font, size) in &mono {
        assert_eq!(font.family.as_ref(), FONT_MONO);
        assert_eq!(*size, MONO_SM.size);
    }
    for (font, size) in fonts(&mds[1], cx) {
        assert_eq!(font.family.as_ref(), FONT_SANS);
        assert_eq!(size, LABEL.size);
    }
    // No paragraph spacing: two lines are exactly two line heights, and the view is as
    // tall as its text.
    let unit_h = mds[0].read_with(cx, |m, _| m.shared.borrow().frame[0].bounds.size.height);
    assert_eq!(unit_h, px(2. * MONO_SM.line));
    let Some(view) = cx.debug_bounds("view-0") else {
        panic!("view not laid out")
    };
    assert_eq!(view.size.height, unit_h);
}

#[gpui::test]
fn the_style_options_work_in_both_modes(cx: &mut TestAppContext) {
    let (mds, cx) = open(cx, Place::Stack, |_, cx| {
        vec![
            cx.new(|cx| Markdown::verbatim("plain", cx)),
            cx.new(|cx| Markdown::new("*Small* paragraph", cx).type_style(LABEL)),
            cx.new(|cx| Markdown::new("Mono paragraph", cx).type_style(MONO_SM)),
        ]
    });
    let fonts = |md: &Entity<Markdown>, cx: &mut VisualTestContext| {
        md.read_with(cx, |m, _| m.shared.borrow().frame[0].layout.fonts())
    };
    let body = fonts(&mds[0], cx);
    assert!(
        body.iter()
            .all(|(f, s)| f.family.as_ref() == FONT_SANS && *s == BODY.size)
    );
    let small = fonts(&mds[1], cx);
    assert!(
        small
            .iter()
            .all(|(f, s)| f.family.as_ref() == FONT_SANS && *s == LABEL.size)
    );
    assert!(
        small
            .iter()
            .any(|(f, _)| f.style == gpui::FontStyle::Italic)
    );
    let mono = fonts(&mds[2], cx);
    assert!(
        mono.iter()
            .all(|(f, s)| f.family.as_ref() == FONT_MONO && *s == MONO_SM.size)
    );
}

const LONG: &str = "error: the agent stopped because the worktree at \
    /home/someone/.harness/worktrees/pipeliner/issue-1234-retry-budget could not be checked out: \
    a3f9c2e17b5d4e0f8a6c1b2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f is not a commit";

fn row_layout(cx: &mut TestAppContext, as_col: bool) {
    let (md, cx) = one(cx, Place::Row { col: as_col }, |cx| {
        Markdown::verbatim(LONG, cx).type_style(LABEL).color(DANGER)
    });
    let Some(column) = cx.debug_bounds("text-col") else {
        panic!("text column not laid out")
    };
    assert_eq!(column.size.width, px(300.));
    let (unit, lines) = md.read_with(cx, |m, _| {
        let s = m.shared.borrow();
        (s.frame[0].bounds, s.frame[0].layout.line_texts().len())
    });
    assert_eq!(unit.size.width, px(300.));
    assert_eq!(unit.origin.x, column.origin.x);
    assert!(lines > 1, "{lines} lines");
    assert!(unit.size.height > px(LABEL.line), "{:?}", unit.size.height);
    assert!(
        unit.size.height < px(LABEL.line * 20.),
        "{:?}",
        unit.size.height
    );
    assert_eq!(unit.size.height, px(LABEL.line * lines as f32));
    // The column, and what comes after the row, get the wrapped height.
    assert_eq!(column.size.height, unit.size.height);
    let Some(below) = cx.debug_bounds("below") else {
        panic!("row below not laid out")
    };
    assert_eq!(below.origin.y, column.origin.y + column.size.height);
}

#[gpui::test]
fn wraps_in_a_flex_column_next_to_a_fixed_sibling(cx: &mut TestAppContext) {
    row_layout(cx, true);
}

#[gpui::test]
fn wraps_in_a_flex_div_next_to_a_fixed_sibling(cx: &mut TestAppContext) {
    row_layout(cx, false);
}

#[gpui::test]
fn selecting_in_another_view_drops_the_first_selection(cx: &mut TestAppContext) {
    let (mds, cx) = open(cx, Place::Stack, |_, cx| {
        vec![
            cx.new(|cx| Markdown::verbatim("alpha beta", cx)),
            cx.new(|cx| Markdown::new("gamma **delta**", cx)),
        ]
    });
    let (a, b) = (mds[0].clone(), mds[1].clone());
    let (from, to) = (point_of(&a, cx, 0), point_of(&a, cx, "alpha".len()));
    drag(cx, from, to);
    assert_eq!(
        a.read_with(cx, |m, _| m.selected_text()).as_deref(),
        Some("alpha")
    );
    let (from, to) = (
        point_of(&b, cx, "gamma ".len()),
        point_of(&b, cx, "gamma del".len()),
    );
    drag(cx, from, to);
    assert_eq!(a.read_with(cx, |m, _| m.selected_text()), None);
    assert_eq!(
        b.read_with(cx, |m, _| m.selected_text()).as_deref(),
        Some("del")
    );
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("del"));
}
