//! "Needs you" and "Working now": rows, what a click opens, the keyboard, empty and failed blocks.

use gpui::TestAppContext;
use hub::api::{InboxId, IssueKey, ReviewId};
use hub::{InboxFocus, Route};

use super::{click, keys, open, open_told, open_with, route, shown};

#[gpui::test]
async fn needs_you_has_a_row_per_need(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    for ix in 0..4 {
        assert!(shown(cx, &format!("home-need-{ix}")), "row {ix}");
    }
    assert!(!shown(cx, "home-need-4"));
    assert!(shown(cx, "home-open-inbox"));
}

#[gpui::test]
async fn a_permission_opens_its_chat(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-need-0");
    assert!(matches!(route(cx), Route::Chat { .. }), "{:?}", route(cx));
}

#[gpui::test]
async fn a_review_opens_in_the_inbox(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-need-1");
    assert_eq!(route(cx), Route::Inbox { item: Some(InboxFocus::Review(ReviewId::new("review-1423"))) });
}

#[gpui::test]
async fn a_question_opens_its_inbox_item(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-need-2");
    assert_eq!(route(cx), Route::Inbox { item: Some(InboxFocus::Item(InboxId::new("inbox-1435"))) });
}

#[gpui::test]
async fn the_keyboard_moves_through_the_needs_and_opens_one(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    keys(cx, "j j");
    assert_eq!(t.view.read_with(cx, |v, _| v.selected), Some(1));
    keys(cx, "k k k");
    assert_eq!(t.view.read_with(cx, |v, _| v.selected), Some(0), "the top stops the selection");
    keys(cx, "j j j j j");
    assert_eq!(t.view.read_with(cx, |v, _| v.selected), Some(3), "so does the bottom");
    keys(cx, "enter");
    assert_eq!(route(cx), Route::Inbox { item: Some(InboxFocus::Review(ReviewId::new("review-1431"))) });
}

#[gpui::test]
async fn working_now_has_a_row_per_run(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    for ix in 0..6 {
        assert!(shown(cx, &format!("home-working-{ix}")), "row {ix}");
    }
    assert!(!shown(cx, "home-working-6"));
}

#[gpui::test]
async fn an_orchestrator_row_opens_its_issue(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-working-0");
    assert_eq!(route(cx), Route::Issues { issue: Some(IssueKey::new("PAY-1436")) });
}

#[gpui::test]
async fn a_chat_row_opens_its_chat(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-working-4");
    assert!(matches!(route(cx), Route::Chat { .. }), "{:?}", route(cx));
}

#[gpui::test]
async fn a_pipeline_row_opens_its_run(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-working-1");
    assert_ne!(route(cx), Route::Home, "a run opens its issue or its pipeline");
}

#[gpui::test]
async fn the_inbox_footer_leads_to_the_inbox(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-open-inbox");
    assert_eq!(route(cx), Route::Inbox { item: None });
}

#[gpui::test]
async fn the_agents_footer_leads_to_the_library(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    click(cx, "home-open-agents");
    assert_eq!(route(cx), Route::Agents { runnable: None });
}

#[gpui::test]
async fn empty_blocks_say_so(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, |world| {
        world.ops.needs.clear();
        world.ops.working.clear();
        world.ops.by_runnable.clear();
    });
    assert!(shown(cx, "home-needs-empty"));
    assert!(shown(cx, "home-working-empty"));
    assert!(!shown(cx, "home-need-0"));
    assert!(!shown(cx, "home-working-0"));
    assert!(!shown(cx, "home-agent-0"));
}

#[gpui::test]
async fn a_failed_block_shows_its_error_and_retry_asks_again(cx: &mut TestAppContext) {
    let failing = ["home_stats", "needs_you", "working_now", "spend_today", "spend_buckets", "spend_by_runnable"];
    let (_t, cx) = open_told(cx, |_| {}, &failing);
    for block in ["stats", "needs", "working", "spend", "agents"] {
        assert!(shown(cx, &format!("home-retry-{block}")), "{block}");
    }
    assert!(!shown(cx, "home-need-0"), "a failed block shows no rows");
    assert!(!shown(cx, "home-working-0"));
    click(cx, "home-retry-needs");
    assert!(shown(cx, "home-need-0"), "after the retry the rows are there");
    assert!(shown(cx, "home-working-0"), "and so is every other block");
    assert!(shown(cx, "home-stat-runs"), "the stats strip");
    assert!(shown(cx, "home-spend-chart"), "the spend chart");
}

#[gpui::test]
async fn a_long_question_is_cut_inside_its_state_cell(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, |world| {
        for working in &mut world.ops.working {
            if let hub::api::WorkingState::Waiting { text } = &mut working.state {
                *text = "asks whether it may run the whole integration suite against staging tonight".into();
            }
        }
    });
    assert!(shown(cx, "home-working-question-4"));
    for ix in 0..6 {
        let Some(question) = cx.debug_bounds(super::leak(&format!("home-working-question-{ix}"))) else { continue };
        let row = cx.debug_bounds(super::leak(&format!("home-working-{ix}"))).expect("the row");
        assert!(
            question.right() <= row.right() - gpui::px(ui::theme::SPACE_4) + gpui::px(0.5),
            "row {ix}: the question ends at {:?}, the row at {:?}",
            question.right(),
            row.right()
        );
    }
}

#[gpui::test]
async fn a_long_runtime_name_leaves_its_slots_in_the_card(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, |world| {
        world.runtimes[0].name = "build-box-in-the-basement-under-the-stairs-next-to-the-boiler".into();
    });
    assert!(shown(cx, "home-runtime-0"));
    let row = cx.debug_bounds(super::leak("home-runtime-0")).expect("the row");
    let name = cx.debug_bounds(super::leak("home-runtime-name-0")).expect("the name");
    let slots = cx.debug_bounds(super::leak("home-runtime-slots-0")).expect("the slots");
    assert!(slots.right() <= row.right() - gpui::px(ui::theme::SPACE_4), "the slots stay in the card: {:?}", slots.right());
    assert!(name.right() < slots.left(), "the name gives way to the dot and the slots");
}
