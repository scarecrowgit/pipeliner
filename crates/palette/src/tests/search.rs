//! What the list shows: recent chats, typed queries, chips, the empty state.

use gpui::TestAppContext;
use hub::LoadState;

use super::{NOW, open, settle, titles};
use crate::facets::Facet;
use crate::render::Body;

#[gpui::test]
fn an_empty_query_lists_the_recent_chats_newest_first(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    let expected: Vec<String> = open.api.with_world(|world| {
        let mut sessions: Vec<_> = world.sessions.iter().filter(|s| !s.archived).collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated));
        sessions.iter().map(|s| s.title.clone()).collect()
    });
    let shown = titles(&open.palette, cx);
    assert!(!shown.is_empty());
    assert_eq!(shown.len(), expected.len(), "every chat fits under the limit");
    // Pinned chats lead; the rest follow newest first.
    let mut rest: Vec<String> = open.palette.read_with(cx, |palette, _| {
        let hits = palette.result.as_ref().map(|result| result.hits.clone()).unwrap_or_default();
        palette
            .listing
            .rows
            .iter()
            .filter_map(|&hit| hits.get(hit))
            .filter(|hit| !hit.session.pinned)
            .map(|hit| hit.session.title.clone())
            .collect()
    });
    let unpinned: Vec<String> = expected
        .iter()
        .filter(|title| open.api.with_world(|w| w.sessions.iter().any(|s| &&s.title == title && !s.pinned)))
        .cloned()
        .collect();
    rest.truncate(unpinned.len());
    assert_eq!(rest, unpinned);
    open.palette.read_with(cx, |palette, cx| {
        assert_eq!(palette.state(cx), &LoadState::Ready);
        assert_eq!(palette.selected, 0);
        assert_eq!(palette.listing.groups[0].label, "Pinned");
        assert!(matches!(palette.body_state(cx), Body::Results));
    });
}

#[gpui::test]
fn typing_searches_after_the_pause_and_a_newer_query_wins(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    let all = titles(&open.palette, cx).len();
    cx.simulate_input("retry");
    // Still inside the debounce: the list is the old one, shown as loading.
    assert_eq!(titles(&open.palette, cx).len(), all);
    open.palette.read_with(cx, |palette, cx| assert_eq!(palette.state(cx), &LoadState::Loading));
    cx.executor().advance_clock(hub::models::DEBOUNCE / 2);
    cx.simulate_input("x");
    settle(cx);
    // "retryx" matches nothing, and the earlier "retry" never shows.
    assert!(titles(&open.palette, cx).is_empty());
    open.palette.read_with(cx, |palette, cx| {
        assert_eq!(palette.text(cx), "retryx");
        assert!(matches!(palette.body_state(cx), Body::Empty));
    });

    cx.simulate_keystrokes("backspace");
    settle(cx);
    let found = titles(&open.palette, cx);
    assert!(found.iter().any(|title| title == "Fix double charge on retry"), "{found:?}");
    assert!(found.len() < all);
    open.palette.read_with(cx, |palette, _| {
        let result = palette.result.as_ref().map(|r| r.hits.clone()).unwrap_or_default();
        let fix = result.iter().find(|hit| hit.session.title == "Fix double charge on retry");
        assert!(fix.is_some_and(|hit| !hit.title_highlights.is_empty()), "the match is marked in the title");
    });
}

#[gpui::test]
fn a_chip_narrows_the_results_and_all_brings_them_back(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    let all = titles(&open.palette, cx).len();
    let waiting = open.palette.read_with(cx, |palette, _| palette.result.as_ref().map_or(0, |r| r.facets.waiting));
    assert!(waiting > 0 && (waiting as usize) < all);

    let at = Facet::ALL.iter().position(|f| *f == Facet::Waiting).unwrap_or(0);
    let Some(bounds) = cx.debug_bounds(Box::leak(format!("palette-chip-{at}").into_boxed_str())) else {
        panic!("the Waiting chip was not laid out")
    };
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    settle(cx);
    assert_eq!(titles(&open.palette, cx).len() as u64, waiting);
    open.palette.read_with(cx, |palette, cx| {
        assert!(Facet::Waiting.pressed(palette.query(cx)));
        assert!(palette.result.as_ref().is_some_and(|r| r.hits.iter().all(|h| h.session.state == hub::api::SessionState::Waiting)));
    });

    open.palette.update(cx, |palette, cx| palette.toggle_facet(Facet::All, cx));
    settle(cx);
    assert_eq!(titles(&open.palette, cx).len(), all);
}

#[gpui::test]
fn nothing_matching_shows_the_empty_state(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    cx.simulate_input("zzzqqq");
    settle(cx);
    open.palette.read_with(cx, |palette, cx| {
        assert!(matches!(palette.body_state(cx), Body::Empty));
        assert!(palette.selected_session().is_none());
    });
    // Enter with nothing selected opens nothing and keeps the palette up.
    cx.simulate_keystrokes("enter");
    assert!(super::overlay_open(cx));
    let _ = NOW;
}

#[gpui::test]
fn an_in_scope_word_becomes_a_tag_and_scopes_the_search(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    cx.simulate_input("in:checkout-web ");
    settle(cx);
    open.palette.read_with(cx, |palette, cx| {
        assert_eq!(palette.scope.as_deref(), Some("checkout-web"));
        assert_eq!(palette.text(cx), "");
        assert_eq!(palette.query(cx).repo.as_deref(), Some("checkout-web"));
        assert!(palette.result.as_ref().is_some_and(|r| {
            !r.hits.is_empty() && r.hits.iter().all(|h| h.session.workdir.as_ref().is_some_and(|w| w.repo == "checkout-web"))
        }));
    });
    open.palette.update(cx, |palette, cx| palette.clear_scope(cx));
    settle(cx);
    open.palette.read_with(cx, |palette, cx| assert!(palette.query(cx).repo.is_none()));
}

#[gpui::test]
fn a_collapsed_section_hides_its_rows_and_the_keyboard_skips_them(cx: &mut TestAppContext) {
    let (open, cx) = open(cx);
    let before = open.palette.read_with(cx, |palette, _| palette.listing.rows.len());
    let pinned = open.palette.read_with(cx, |palette, _| palette.listing.groups[0].hits.len());
    assert!(pinned > 0);
    open.palette.update(cx, |palette, cx| palette.toggle_group("Pinned", cx));
    open.palette.read_with(cx, |palette, _| {
        assert_eq!(palette.listing.rows.len(), before - pinned);
        assert!(palette.selected < palette.listing.rows.len());
        assert!(palette.selected_session().is_some_and(|session| !session.pinned));
    });
    open.palette.update(cx, |palette, cx| palette.toggle_group("Pinned", cx));
    open.palette.read_with(cx, |palette, _| assert_eq!(palette.listing.rows.len(), before));
}
