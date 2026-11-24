//! The queue: order, tabs, opening, keys, archive, and the other kinds of item.

use gpui::TestAppContext;
use hub::InboxFocus;
use hub::api::InboxId;

use super::{calls, click, focus, keys, open, open_review, open_with, rows, shown};

fn item(id: &str) -> Option<InboxFocus> {
    Some(InboxFocus::Item(InboxId::new(id)))
}

#[gpui::test]
async fn waiting_items_come_first_and_tabs_filter(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, None);
    assert_eq!(
        rows(&t, cx),
        ["inbox-1423", "inbox-1435", "inbox-1427", "inbox-1431"]
    );
    click(cx, "inbox-tab-all");
    assert_eq!(
        rows(&t, cx),
        [
            "inbox-1423",
            "inbox-1435",
            "inbox-1427",
            "inbox-1431",
            "inbox-1419"
        ]
    );
    click(cx, "inbox-tab-archived");
    assert!(rows(&t, cx).is_empty());
    assert!(shown(cx, "inbox-empty"));
}

#[gpui::test]
async fn with_no_focus_the_first_waiting_item_opens(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, None);
    assert_eq!(focus(&t, cx), item("inbox-1423"));
    assert_eq!(open_review(&t, cx).as_deref(), Some("review-1423"));
    // Opening marks the item read, once.
    assert_eq!(calls(&t.api, "set_inbox_read"), ["inbox-1423 true"]);
}

#[gpui::test]
async fn clicking_a_row_opens_it_and_follows_the_route(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, None);
    click(cx, "inbox-row-2");
    assert_eq!(focus(&t, cx), item("inbox-1427"));
    assert_eq!(open_review(&t, cx).as_deref(), Some("review-1427"));
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(
        route,
        hub::Route::Inbox {
            item: item("inbox-1427")
        }
    );
    let ready = t.view.read_with(cx, |v, cx| v.open_review(cx).is_some());
    assert!(ready, "the review loaded");
    assert!(shown(cx, "review-variant-1"));
}

#[gpui::test]
async fn keys_move_through_the_queue(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, None);
    keys(cx, "j");
    assert_eq!(focus(&t, cx), item("inbox-1435"));
    assert_eq!(open_review(&t, cx), None, "a question has no review");
    keys(cx, "down");
    assert_eq!(focus(&t, cx), item("inbox-1427"));
    keys(cx, "k");
    assert_eq!(focus(&t, cx), item("inbox-1435"));
    keys(cx, "up up");
    assert_eq!(
        focus(&t, cx),
        item("inbox-1423"),
        "the top stops the selection"
    );
    keys(cx, "2");
    let chosen = t
        .view
        .read_with(cx, |v, _| v.review.as_ref().and_then(|r| r.variant));
    assert_eq!(chosen, Some(2));
    keys(cx, "r");
    assert!(shown(cx, "send-back-note"));
}

#[gpui::test]
async fn archiving_moves_to_the_next_item(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, item("inbox-1435"));
    click(cx, "inbox-archive");
    assert_eq!(calls(&t.api, "archive_inbox"), ["inbox-1435 true"]);
    assert_eq!(focus(&t, cx), item("inbox-1427"));
    assert_eq!(rows(&t, cx), ["inbox-1423", "inbox-1427", "inbox-1431"]);
    click(cx, "inbox-tab-archived");
    assert_eq!(rows(&t, cx), ["inbox-1435"]);
}

#[gpui::test]
async fn mark_unread_sticks_while_the_item_stays_open(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, item("inbox-1435"));
    click(cx, "inbox-read");
    assert_eq!(
        calls(&t.api, "set_inbox_read"),
        ["inbox-1435 true", "inbox-1435 false"]
    );
    let read = t
        .view
        .read_with(cx, |v, cx| v.current_item(cx).map(|item| item.read));
    assert_eq!(read, Some(false));
}

#[gpui::test]
async fn a_question_links_to_where_it_is_answered(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx, item("inbox-1435"));
    assert!(shown(cx, "item-summary"));
    assert!(shown(cx, "item-answer"));
    click(cx, "item-answer");
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(
        route,
        hub::Route::Issues {
            issue: Some(hub::api::IssueKey::new("PAY-1435"))
        }
    );
}

#[gpui::test]
async fn a_report_shows_its_pull_request_status(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, item("inbox-1419"));
    assert!(shown(cx, "item-pull-request"));
    let merged = t.view.read_with(cx, |v, _| {
        v.prs.values().any(
            |status| matches!(status, crate::view::PrStatus::Ready(pr) if pr.state == "merged"),
        )
    });
    assert!(merged);
}

#[gpui::test]
async fn a_failure_shows_the_error_and_the_run(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, item("inbox-fail"), |world| {
        let mut failure = world.inbox[0].clone();
        failure.id = InboxId::new("inbox-fail");
        failure.kind = hub::api::InboxKind::Failure;
        failure.summary = "Budget of $2.00 reached".to_string();
        failure.created = super::NOW;
        world.inbox.push(failure);
    });
    assert!(shown(cx, "item-error"));
    assert!(shown(cx, "item-open-run"));
}

#[gpui::test]
async fn an_empty_inbox_says_nothing_waits(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, None, |world| world.inbox.clear());
    assert!(shown(cx, "inbox-empty"));
    assert!(shown(cx, "inbox-nothing"));
}
