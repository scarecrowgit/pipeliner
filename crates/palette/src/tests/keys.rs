//! Keyboard and pointer: moving, opening, closing, commands.

use gpui::{Modifiers, TestAppContext};
use hub::Route;
use hub::api::SessionId;

use super::{open, overlay_open, route, settle};

fn selected(open: &super::Open, cx: &mut gpui::VisualTestContext) -> Option<SessionId> {
    open.palette.read_with(cx, |palette, _| palette.selected_session().map(|s| s.id.clone()))
}

#[gpui::test]
fn arrows_move_and_wrap_and_pages_stop_at_the_ends(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    let rows = open.palette.read_with(cx, |palette, _| palette.listing.rows.len());
    assert!(rows > 3);
    cx.simulate_keystrokes("down down");
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.selected, 2));
    cx.simulate_keystrokes("up up");
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.selected, 0));
    cx.simulate_keystrokes("up");
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.selected, rows - 1, "Up from the first row wraps"));
    cx.simulate_keystrokes("down");
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.selected, 0, "Down from the last row wraps"));

    cx.simulate_keystrokes("pagedown");
    let after_page = open.palette.read_with(cx, |palette, _| palette.selected);
    assert!(after_page > 1 && after_page < rows);
    for _ in 0..rows {
        cx.simulate_keystrokes("pagedown");
    }
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.selected, rows - 1, "pages stop at the last row"));
    for _ in 0..rows {
        cx.simulate_keystrokes("pageup");
    }
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.selected, 0, "pages stop at the first row"));
}

#[gpui::test]
fn enter_opens_the_selected_chat_and_clears_the_overlay(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    cx.simulate_keystrokes("down down");
    let Some(chosen) = selected(&open, cx) else { panic!("nothing selected") };
    assert!(overlay_open(cx));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(route(cx), Route::Chat { session: chosen });
    assert!(!overlay_open(cx));
}

#[gpui::test]
fn escape_closes_without_navigating_or_clearing_the_query(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    cx.simulate_input("retry");
    let before = route(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!overlay_open(cx));
    assert_eq!(route(cx), before);
    open.palette.read_with(cx, |palette, cx| assert_eq!(palette.text(cx), "retry"));

    // Once closing, further keys do nothing.
    cx.simulate_keystrokes("down enter");
    assert_eq!(route(cx), before);
}

#[gpui::test]
fn the_pointer_moves_the_selection_and_a_click_opens(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    let Some(bounds) = cx.debug_bounds("palette-row-3") else { panic!("row 3 was not laid out") };
    cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.selected, 3));
    let Some(chosen) = selected(&open, cx) else { panic!("nothing selected") };
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(route(cx), Route::Chat { session: chosen });
    assert!(!overlay_open(cx));
}

#[gpui::test]
fn a_press_on_the_scrim_closes(cx: &mut TestAppContext) {
    let (_open, cx) = open(cx);
    cx.simulate_click(gpui::point(gpui::px(4.), gpui::px(4.)), Modifiers::none());
    cx.run_until_parked();
    assert!(!overlay_open(cx));
}

#[gpui::test]
fn pin_and_archive_go_to_the_engine_and_the_list_follows(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    // A chat that is not pinned.
    let target = open.palette.read_with(cx, |palette, _| {
        let hits = palette.result.as_ref().map(|r| r.hits.clone()).unwrap_or_default();
        palette.listing.rows.iter().position(|&hit| hits.get(hit).is_some_and(|h| !h.session.pinned))
    });
    let Some(row) = target else { panic!("every chat is pinned") };
    open.palette.update(cx, |palette, cx| palette.select(row, true, cx));
    let Some(id) = selected(&open, cx) else { panic!("nothing selected") };

    cx.simulate_keystrokes("ctrl-p");
    settle(cx);
    assert!(open.api.recorded().iter().any(|c| c.name == "pin_session"));
    let pinned = open.api.with_world(|w| w.sessions.iter().any(|s| s.id == id && s.pinned));
    assert!(pinned);
    open.palette.read_with(cx, |palette, _| {
        let first = palette.result.as_ref().and_then(|r| r.hits.iter().find(|h| h.session.id == id));
        assert!(first.is_some_and(|hit| hit.session.pinned), "the list was reloaded");
    });

    // Archive with the field empty: the chat leaves the list.
    open.palette.update(cx, |palette, cx| {
        let row = palette
            .listing
            .rows
            .iter()
            .position(|&hit| palette.result.as_ref().and_then(|r| r.hits.get(hit)).is_some_and(|h| h.session.id == id))
            .unwrap_or(0);
        palette.select(row, true, cx)
    });
    cx.simulate_keystrokes("cmd-backspace");
    settle(cx);
    assert!(open.api.recorded().iter().any(|c| c.name == "archive_session"));
    open.palette.read_with(cx, |palette, _| {
        assert!(palette.result.as_ref().is_some_and(|r| r.hits.iter().all(|h| h.session.id != id)));
    });
}

#[gpui::test]
fn archive_does_nothing_while_something_is_typed(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    cx.simulate_input("retry");
    settle(cx);
    cx.simulate_keystrokes("cmd-backspace");
    settle(cx);
    assert!(!open.api.recorded().iter().any(|c| c.name == "archive_session"));
    open.palette.read_with(cx, |palette, cx| assert_eq!(palette.text(cx), ""), );
}
