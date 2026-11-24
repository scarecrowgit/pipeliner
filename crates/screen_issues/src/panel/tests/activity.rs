//! The activity feed: every kind of event, wake-ups inside the comment that caused them, retry,
//! and Delete on one's own comments.

use gpui::TestAppContext;
use hub::api::{Actor, IssueEvent, IssueEventKind, IssueStatus, RunnableRef};
use hub::model::IssueKey;

use super::{NOW, calls, click, hover, open, open_with, shown};

#[gpui::test]
fn the_feed_renders_every_event_of_the_fixture(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, "PAY-1427", |w| {
        let feed = w.issue_events.entry(IssueKey::new("PAY-1427")).or_default();
        let mut push = |actor: Actor, kind: IssueEventKind| {
            let seq = feed.len() as u64 + 1;
            feed.push(IssueEvent { seq, at: NOW - 60_000, actor, kind });
        };
        push(Actor::Me, IssueEventKind::StatusChanged { from: IssueStatus::Todo, to: IssueStatus::InProgress });
        push(Actor::Me, IssueEventKind::Detached);
        let fixer = Actor::Runnable { reference: RunnableRef::agent("fixer"), name: "Fixer".into(), spawned: false };
        push(fixer, IssueEventKind::PrOpened { url: "https://github.com/acme/payments-api/pull/2291".into() });
        push(
            Actor::System,
            IssueEventKind::Woken { target: RunnableRef::agent("analyst"), name: "Analyst".into(), run: None, number: 3, cause: "moved to Todo".into() },
        );
    });
    let len = t.api.with_world(|w| w.issue_events[&IssueKey::new("PAY-1427")].len());
    assert_eq!(len, 13);
    for ix in 0..len {
        assert!(shown(cx, &format!("issue-activity-{ix}")), "event {ix} is drawn");
    }
    assert!(shown(cx, "issue-activity-11-pr"), "the pull request links out");
}

#[gpui::test]
fn a_new_event_appears_at_the_bottom(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    assert!(!shown(cx, "issue-activity-9"));
    t.api.emit(hub::api::Event::IssueActivity {
        key: IssueKey::new("PAY-1427"),
        event: IssueEvent { seq: 10, at: NOW, actor: Actor::Me, kind: IssueEventKind::Detached },
    });
    assert!(shown(cx, "issue-activity-9"));
}

#[gpui::test]
fn a_failed_wake_up_can_be_retried(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    assert!(shown(cx, "issue-activity-8"), "the failure sits inside the comment that caused it");
    click(cx, "issue-retry-wake-8");
    assert_eq!(calls(&t.api, "retry_wake"), ["PAY-1427 agent:codex"]);
}

#[gpui::test]
fn delete_shows_on_ones_own_comments_only(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx, "PAY-1427");
    assert!(shown(cx, "issue-comment-delete-4"), "your comment has Delete");
    assert!(!shown(cx, "issue-comment-delete-6"), "Fixer's comment has none");
    hover(cx, "issue-activity-4");
    click(cx, "issue-comment-delete-4");
    let toast = cx.update(|window, cx| ui::overlays(window, cx).has_toast(cx));
    assert!(!toast, "deleting is a command now, not a toast");
}

#[gpui::test]
fn deleting_a_comment_drops_it_from_the_feed(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    let key = IssueKey::new("PAY-1427");
    let (seq, before) = t.api.with_world(|w| {
        let feed = &w.issue_events[&key];
        (feed[4].seq, feed.len())
    });
    assert!(shown(cx, "issue-comment-delete-4"));
    hover(cx, "issue-activity-4");
    click(cx, "issue-comment-delete-4");
    assert_eq!(calls(&t.api, "delete_comment"), [format!("PAY-1427 {seq}")]);
    let seqs: Vec<u64> = t.panel.read_with(cx, |panel, cx| {
        let feed = panel.issues.read(cx).activity(&panel.issue).expect("the feed is loaded");
        feed.value().expect("a loaded feed").iter().map(|e| e.seq).collect()
    });
    assert_eq!(seqs.len(), before - 1);
    assert!(!seqs.contains(&seq), "the deleted comment left the feed");
}
