//! Selecting and copying a diff's code: what a copy gives (the model, without a window) and
//! the mouse and keys in a test window, next to a Markdown view.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, ElementId, Entity, IntoElement, Modifiers, MouseButton, Pixels, Point, Render,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};

use super::super::model::diff_rows_with_sources;
use super::super::select::{Shared, row_point, shared_of};
use super::*;
use crate::markdown::element::unit_point;
use crate::markdown::selection::TextPoint;
use crate::markdown::{Markdown, bind_keys};

/// A context line, a removed and an added line, a context line; a second hunk with a tab
/// and a line too long to show whole.
fn file() -> DiffFile {
    let h1 = DiffHunk {
        lines: vec![
            ctx(10, 10, "fn charge(amount: u32) {"),
            del(11, "    let fee = amount / 10;"),
            add(11, "    let fee = amount / 20;"),
            ctx(12, 12, "    pay(amount + fee)"),
        ],
    };
    let h2 = DiffHunk {
        lines: vec![
            add(40, "\tindented();"),
            add(41, &format!("let s = \"{}\";", "x".repeat(MAX_LINE))),
        ],
    };
    DiffFile {
        path: "src/charge.rs".into(),
        added: 3,
        removed: 1,
        hunks: vec![h1, h2],
        ..Default::default()
    }
}

fn shared_for(file: DiffFile) -> Shared {
    let (rows, sources) = diff_rows_with_sources(&file);
    let mut s = Shared {
        shown: rows.len(),
        ..Default::default()
    };
    s.set_file(Arc::new(file), rows, sources);
    s
}

fn select(s: &mut Shared, from: (usize, usize), to: (usize, usize)) -> Option<String> {
    s.sel.anchor = Some(TextPoint {
        unit: from.0,
        offset: from.1,
    });
    s.sel.head = Some(TextPoint {
        unit: to.0,
        offset: to.1,
    });
    s.selected_text()
}

#[test]
fn a_copy_gives_the_source_code_of_the_rows() {
    let mut s = shared_for(file());
    // Rows: 0..4 the first hunk, 4 the separator, 5 the tab line, 6 the long line.
    assert!(matches!(s.rows[4], DiffRow::Separator { .. }));
    // Backwards works too; the separator gives nothing, the tab comes back as a tab.
    let text = select(&mut s, (5, "    indented".len()), (3, "    pay".len()));
    assert_eq!(text.as_deref(), Some("(amount + fee)\n\tindented"));
    // Part of a shown tab copies the tab.
    assert_eq!(select(&mut s, (5, 2), (5, 5)).as_deref(), Some("\ti"));
    // Selected to its end, a line cut for display is copied whole.
    let long = match &s.rows[6] {
        DiffRow::Line { text, .. } => text.len(),
        _ => 0,
    };
    let text = select(&mut s, (6, 0), (6, long)).unwrap_or_default();
    assert_eq!(text.len(), MAX_LINE + "let s = \"\";".len());
    // Nothing selected, nothing copied.
    assert_eq!(select(&mut s, (1, 3), (1, 3)), None);
    // Select all covers the rows shown.
    s.shown = 2;
    s.select_all();
    assert_eq!(
        s.selected_text().as_deref(),
        Some("fn charge(amount: u32) {\n    let fee = amount / 10;")
    );
}

/// A diff card and a Markdown view under it, then a marker row.
struct Host {
    file: Arc<DiffFile>,
    md: Entity<Markdown>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().child(
            div()
                .w(px(720.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    DiffCard::new("edit", self.file.clone())
                        .compact()
                        .collapsible(),
                )
                .child(self.md.clone())
                .child(div().h(px(10.)).w_full().debug_selector(|| "below".into())),
        )
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<Markdown>, &mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        bind_keys(cx);
    });
    let slot: Rc<RefCell<Option<Entity<Markdown>>>> = Rc::default();
    let s = slot.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        let md = cx.new(|cx| Markdown::new("gamma **delta**", cx));
        *s.borrow_mut() = Some(md.clone());
        Host {
            file: Arc::new(file()),
            md,
        }
    });
    cx.run_until_parked();
    let Some(md) = slot.borrow().clone() else {
        panic!("no markdown view")
    };
    (md, cx)
}

fn view_id() -> ElementId {
    ElementId::NamedChild(Box::new("edit".into()), "view".into())
}

fn diff(cx: &mut VisualTestContext) -> Option<Rc<RefCell<Shared>>> {
    cx.run_until_parked();
    shared_of(&view_id())
}

fn diff_text(cx: &mut VisualTestContext) -> Option<String> {
    diff(cx).and_then(|s| s.borrow().selected_text())
}

fn at(cx: &mut VisualTestContext, row: usize, offset: usize) -> Point<Pixels> {
    let Some(s) = diff(cx) else {
        panic!("diff not shown")
    };
    row_point(&s.borrow(), row, offset).unwrap_or_else(|| panic!("row {row} not laid out"))
}

fn md_at(md: &Entity<Markdown>, cx: &mut VisualTestContext, offset: usize) -> Point<Pixels> {
    md.read_with(cx, |m, _| unit_point(&m.shared.borrow(), 0, offset))
        .unwrap_or_else(|| panic!("offset {offset} not laid out"))
}

fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text())
}

#[gpui::test]
fn a_drag_across_lines_copies_the_code_without_markers(cx: &mut TestAppContext) {
    let (_, cx) = open(cx);
    // From the start of the context line to the end of the added line: whole lines.
    let (from, to) = (at(cx, 0, 0), at(cx, 2, "    let fee = amount / 20;".len()));
    drag(cx, from, to);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(
        clipboard(cx).as_deref(),
        Some("fn charge(amount: u32) {\n    let fee = amount / 10;\n    let fee = amount / 20;")
    );
    // Mid-line to mid-line: a partial first and last line.
    let (from, to) = (at(cx, 0, "fn ".len()), at(cx, 2, "    let fee".len()));
    drag(cx, from, to);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(
        clipboard(cx).as_deref(),
        Some("charge(amount: u32) {\n    let fee = amount / 10;\n    let fee")
    );
}

#[gpui::test]
fn a_drag_inside_a_line_copies_the_substring(cx: &mut TestAppContext) {
    let (_, cx) = open(cx);
    let (from, to) = (
        at(cx, 1, "    let ".len()),
        at(cx, 1, "    let fee = amount".len()),
    );
    drag(cx, from, to);
    assert_eq!(diff_text(cx).as_deref(), Some("fee = amount"));
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("fee = amount"));
    // Select all, then copy: every row's code.
    cx.simulate_keystrokes("ctrl-a");
    let all = diff_text(cx).unwrap_or_default();
    assert!(all.starts_with("fn charge(amount: u32) {\n"), "{all}");
    assert!(all.contains("pay(amount + fee)\n\tindented();\n"), "{all}");
}

#[gpui::test]
fn the_header_folds_the_file_and_does_not_select(cx: &mut TestAppContext) {
    let (_, cx) = open(cx);
    let Some(toggle) = cx.debug_bounds("edit-toggle") else {
        panic!("no toggle")
    };
    // A drag from the header's path into the code selects nothing.
    let path = gpui::point(toggle.origin.x - px(400.), toggle.center().y);
    let to = at(cx, 2, 6);
    drag(cx, path, to);
    assert_eq!(diff_text(cx), None);
    // The chevron still folds the body away.
    let before = cx.debug_bounds("below").map(|b| b.origin.y);
    cx.simulate_click(toggle.center(), Modifiers::none());
    cx.run_until_parked();
    let after = cx.debug_bounds("below").map(|b| b.origin.y);
    let (Some(before), Some(after)) = (before, after) else {
        panic!("marker not laid out")
    };
    assert!(after < before - px(100.), "{before:?} -> {after:?}");
    assert_eq!(diff_text(cx), None);
}

#[gpui::test]
fn one_selection_at_a_time_between_markdown_and_a_diff(cx: &mut TestAppContext) {
    let (md, cx) = open(cx);
    let (from, to) = (at(cx, 3, 4), at(cx, 3, "    pay".len()));
    drag(cx, from, to);
    assert_eq!(diff_text(cx).as_deref(), Some("pay"));
    // A press in the Markdown view drops the diff's selection; copy acts on Markdown.
    let (from, to) = (
        md_at(&md, cx, "gamma ".len()),
        md_at(&md, cx, "gamma del".len()),
    );
    drag(cx, from, to);
    assert_eq!(diff_text(cx), None);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("del"));
    // And the other way round.
    let (from, to) = (at(cx, 0, 0), at(cx, 0, "fn".len()));
    drag(cx, from, to);
    assert_eq!(md.read_with(cx, |m, _| m.selected_text()), None);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard(cx).as_deref(), Some("fn"));
}
