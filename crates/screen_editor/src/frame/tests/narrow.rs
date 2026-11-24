//! The editor in a narrow window (728px, a 960px window beside the open sidebar) and in a
//! wide one: every control stays inside the editor, the toolbar keeps one row, and the texts
//! of the Problems tab can be selected and copied.

use gpui::{Bounds, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px, size};
use hub::model::NodeId;
use ui::theme::PANEL_WIDTH;

use super::{click, leak, open, wait_check};
use crate::Selection;
use crate::canvas::NewNode;
use crate::frame::DockTab;

/// The editor area of a 960px window with the 232px sidebar open.
const NARROW: f32 = 728.0;
/// The editor area of a 1716px window with the sidebar open.
const WIDE: f32 = 1484.0;

fn resize(cx: &mut VisualTestContext, width: f32) {
    cx.simulate_resize(size(px(width), px(1060.)));
    cx.run_until_parked();
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"))
}

/// `inner` lies inside `outer`, give or take half a pixel.
fn assert_inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>, what: &str) {
    let slack = px(0.5);
    assert!(
        inner.left() >= outer.left() - slack
            && inner.right() <= outer.right() + slack
            && inner.top() >= outer.top() - slack
            && inner.bottom() <= outer.bottom() + slack,
        "{what} {inner:?} is outside {outer:?}"
    );
}

/// The selectors of every toolbar control.
fn toolbar_controls() -> Vec<String> {
    let mut all: Vec<String> = NewNode::ALL.iter().map(|kind| format!("toolbar-add-{}", kind.key())).collect();
    all.extend(["editor-zoom-out", "editor-zoom-in", "editor-fit"].map(String::from));
    all
}

#[gpui::test]
async fn a_narrow_editor_keeps_every_control_inside(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    resize(cx, NARROW);
    let editor = bounds(cx, "editor");
    assert_eq!(editor.size.width, px(NARROW));
    assert!(t.view.read_with(cx, |v, _| v.toolbar_compact()), "the chips drop their labels");
    for control in toolbar_controls() {
        assert_inside(bounds(cx, &control), editor, &control);
    }
    // The dock lies left of the inspector; its five figures stay inside it.
    let mut dock = editor;
    dock.size.width -= px(PANEL_WIDTH);
    t.view.read_with(cx, |v, _| assert_eq!(v.dock_tab(), DockTab::TestRun));
    for ix in 0..5 {
        let figure = format!("test-run-figure-{ix}");
        let b = bounds(cx, &figure);
        assert_inside(b, dock, &figure);
        assert!(b.size.width >= px(90.), "{figure} is {:?} wide", b.size.width);
    }
    // The header leaves out the action that does nothing yet.
    let actions = t.view.update_in(cx, |v, window, cx| v.header_actions(window, cx).len());
    assert_eq!(actions, 4, "undo, redo, test run, deploy");
    for button in ["editor-test-run", "editor-deploy"] {
        assert!(bounds(cx, button).right() <= px(NARROW), "{button}");
    }
}

#[gpui::test]
async fn a_wide_editor_keeps_the_labelled_toolbar_on_one_row(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    resize(cx, WIDE);
    let editor = bounds(cx, "editor");
    assert!(!t.view.read_with(cx, |v, _| v.toolbar_compact()));
    let first = bounds(cx, "toolbar-add-source");
    for control in toolbar_controls() {
        let b = bounds(cx, &control);
        assert_inside(b, editor, &control);
        assert_eq!(b.center().y, first.center().y, "{control} is on the toolbar's row");
    }
    // Labelled chips are wider than an icon.
    assert!(bounds(cx, "toolbar-add-orchestrator").size.width > px(60.));
    // The five figures share one row.
    let top = bounds(cx, "test-run-figure-0").top();
    for ix in 1..5 {
        assert_eq!(bounds(cx, &format!("test-run-figure-{ix}")).top(), top);
    }
    let actions = t.view.update_in(cx, |v, window, cx| v.header_actions(window, cx).len());
    assert_eq!(actions, 5);
}

/// Presses at `from`, drags to `to` and lets go.
pub(super) fn drag(cx: &mut VisualTestContext, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>) {
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
}

/// Drags over the whole text drawn at `selector` (from its first glyph to the end of its
/// last line) and copies it; returns the clipboard.
pub(super) fn drag_and_copy(cx: &mut VisualTestContext, selector: &str) -> Option<String> {
    let b = bounds(cx, selector);
    let from = point(b.left() + px(1.), b.top() + px(4.));
    let to = point(b.right() - px(1.), b.bottom() - px(4.));
    drag(cx, from, to);
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();
    cx.read_from_clipboard().and_then(|item| item.text())
}

#[gpui::test]
async fn a_problem_can_be_copied_and_a_click_still_selects_its_node(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    wait_check(cx);
    click(cx, "dock-tabs-2");
    let (message, node) = t.view.read_with(cx, |v, cx| {
        let problem = v.listed_problems(cx).into_iter().next().expect("PAY triage has a problem");
        (problem.message, problem.node)
    });
    assert_eq!(drag_and_copy(cx, "problem-0-message"), Some(message));
    t.view.read_with(cx, |v, cx| {
        assert_eq!(v.draft().read(cx).selection(), &Selection::Node(NodeId::new("fixer")), "a drag is not a click");
    });
    click(cx, "problem-0");
    let node = node.expect("the problem is about a node");
    t.view.read_with(cx, |v, cx| assert_eq!(v.draft().read(cx).selection(), &Selection::Node(node)));
}

#[gpui::test]
async fn a_problem_wraps_in_a_narrow_editor(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    resize(cx, NARROW);
    wait_check(cx);
    click(cx, "dock-tabs-2");
    let mut dock = bounds(cx, "editor");
    dock.size.width -= px(PANEL_WIDTH);
    let count = t.view.read_with(cx, |v, cx| v.listed_problems(cx).len());
    for ix in 0..count {
        for part in ["name", "message"] {
            let selector = format!("problem-{ix}-{part}");
            assert_inside(bounds(cx, &selector), dock, &selector);
        }
    }
}
