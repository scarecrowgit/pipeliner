//! The detail is always the selected row of the visible list.

use gpui::{TestAppContext, VisualTestContext};
use hub::InboxFocus;
use hub::api::{InboxId, Link, Outcome};
use hub::fixtures::World;

use super::{Open, click, open, open_with, rows, shown};
use crate::list::Tab;

fn item(id: &str) -> Option<InboxFocus> {
    Some(InboxFocus::Item(InboxId::new(id)))
}

fn tab(t: &Open, cx: &mut VisualTestContext) -> Tab {
    t.view.read_with(cx, |v, _| v.tab)
}

fn focus(t: &Open, cx: &mut VisualTestContext) -> Option<InboxFocus> {
    super::focus(t, cx)
}

/// The real-data case: a pull request report for HAR-7 waits for a review; an older report
/// for HAR-4 is done and does not wait.
fn reports_only(world: &mut World) {
    let read = world
        .inbox
        .iter()
        .find(|i| i.id.as_str() == "inbox-1419")
        .cloned();
    world.inbox.clear();
    let Some(read) = read else { return };
    let mut pr = read.clone();
    pr.id = InboxId::new("inbox-har-7");
    pr.title = "Pull request for HAR-7".to_string();
    pr.read = false;
    pr.outcome = Some(Outcome::NeedsReview);
    pr.created = super::NOW - 60_000;
    pr.links = vec![Link {
        label: "PR #7".to_string(),
        url: "https://github.com/acme/harness/pull/7".to_string(),
    }];
    let mut report = read;
    report.id = InboxId::new("inbox-har-4");
    report.title = "Report for HAR-4".to_string();
    report.links.clear();
    world.inbox.push(pr);
    world.inbox.push(report);
}

#[gpui::test]
async fn with_no_focus_the_first_waiting_row_opens(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, None, reports_only);
    assert_eq!(tab(&t, cx), Tab::Waiting);
    assert_eq!(focus(&t, cx), item("inbox-har-7"));
    assert_eq!(rows(&t, cx), ["inbox-har-7"]);
    let shown_title = t
        .view
        .read_with(cx, |v, cx| v.current_item(cx).map(|i| i.title.clone()));
    assert_eq!(shown_title.as_deref(), Some("Pull request for HAR-7"));
}

#[gpui::test]
async fn with_nothing_waiting_the_all_tab_opens_on_its_first_row(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, None, |world| {
        reports_only(world);
        world.inbox.retain(|i| i.id.as_str() == "inbox-har-4");
    });
    assert_eq!(tab(&t, cx), Tab::All);
    assert_eq!(focus(&t, cx), item("inbox-har-4"));
    assert_eq!(rows(&t, cx), ["inbox-har-4"]);
}

#[gpui::test]
async fn with_nothing_at_all_the_empty_state_shows(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, None, |world| world.inbox.clear());
    assert_eq!(focus(&t, cx), None);
    assert!(shown(cx, "inbox-nothing"));
}

#[gpui::test]
async fn a_routed_item_of_another_tab_shows_that_tab(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, item("inbox-1419"));
    assert_eq!(tab(&t, cx), Tab::All, "a read report does not wait");
    assert!(rows(&t, cx).contains(&"inbox-1419".to_string()));
    assert_eq!(focus(&t, cx), item("inbox-1419"));
}

#[gpui::test]
async fn a_routed_archived_item_shows_the_archive(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-1435"), |world| {
        if let Some(i) = world
            .inbox
            .iter_mut()
            .find(|i| i.id.as_str() == "inbox-1435")
        {
            i.archived = true;
        }
    });
    assert_eq!(tab(&t, cx), Tab::Archived);
    assert_eq!(rows(&t, cx), ["inbox-1435"]);
    assert_eq!(focus(&t, cx), item("inbox-1435"));
}

#[gpui::test]
async fn a_tab_change_opens_its_first_row_unless_the_open_item_is_in_it(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, item("inbox-1427"), |world| {
        if let Some(i) = world
            .inbox
            .iter_mut()
            .find(|i| i.id.as_str() == "inbox-1435")
        {
            i.archived = true;
        }
    });
    assert_eq!(tab(&t, cx), Tab::Waiting);
    click(cx, "inbox-tabs-1");
    assert_eq!(
        focus(&t, cx),
        item("inbox-1427"),
        "the open item is under All too"
    );
    click(cx, "inbox-tabs-2");
    assert_eq!(focus(&t, cx), item("inbox-1435"));
    click(cx, "inbox-tabs-0");
    assert_eq!(focus(&t, cx), item("inbox-1423"));
    // The route follows the selection.
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(
        route,
        hub::Route::Inbox {
            item: item("inbox-1423")
        }
    );
}

#[gpui::test]
async fn an_empty_tab_picked_by_hand_stays_shown(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, None);
    click(cx, "inbox-tabs-2");
    assert_eq!(tab(&t, cx), Tab::Archived);
    assert_eq!(focus(&t, cx), None);
    assert!(rows(&t, cx).is_empty());
}
