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
        let fixer = Actor::Runnable { reference: RunnableRef::agent("fixer"), name: "Fixer".into(), spawned: false, session: None };
        push(fixer, IssueEventKind::PrOpened { url: "https://github.com/acme/payments-api/pull/2291".into() });
        push(
            Actor::System,
            IssueEventKind::Woken { target: RunnableRef::agent("analyst"), name: "Analyst".into(), run: None, number: 3, cause: "moved to Todo".into() },
        );
    });
    let len = t.api.with_world(|w| w.issue_events[&IssueKey::new("PAY-1427")].len());
    assert_eq!(len, 16);
    for ix in 0..len {
        assert!(shown(cx, &format!("issue-activity-{ix}")), "event {ix} is drawn");
    }
    assert!(shown(cx, "issue-activity-14-pr"), "the pull request links out");
}

#[gpui::test]
fn a_new_event_appears_at_the_bottom(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    assert!(!shown(cx, "issue-activity-12"));
    t.api.emit(hub::api::Event::IssueActivity {
        key: IssueKey::new("PAY-1427"),
        event: IssueEvent { seq: 13, at: NOW, actor: Actor::Me, kind: IssueEventKind::Detached },
    });
    assert!(shown(cx, "issue-activity-12"));
}

#[gpui::test]
fn a_run_agents_answer_is_its_comment_with_a_way_into_its_session(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    // The mention (9), the wake inside it (10, seq 11) and the Fixer's answer (11, seq 12).
    assert!(shown(cx, "issue-activity-10-open"), "the wake opens the session it continues");
    let (woke, answer) = t.panel.read_with(cx, |p, cx| {
        let woke = p.texts.get("event-11").map(|v| v.read(cx).text().to_string());
        let answer = p.bodies.get(&12).map(|v| v.read(cx).text().to_string());
        (woke.unwrap_or_default(), answer.unwrap_or_default())
    });
    assert_eq!(woke, "Fixer woken · @mention · continues its work from run 2 in its worktree");
    assert!(answer.starts_with("Rebased `fix/pay-1427-batch` on main"), "{answer}");
    assert!(shown(cx, "issue-activity-11"), "the answer is a row of the Fixer");
    click(cx, "issue-activity-11-session");
    let route = cx.update(|_, cx| hub::router(cx).read(cx).current().clone());
    assert_eq!(route, hub::Route::Chat { session: hub::api::SessionId::new(hub::fixtures::FIXER_1427) });
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

#[gpui::test]
fn a_comment_image_opens_the_viewer_and_esc_closes_only_the_viewer(cx: &mut TestAppContext) {
    // PAY-1440's second event is your comment with the nightly report attached.
    let (t, cx) = open(cx, "PAY-1440");
    assert!(shown(cx, "issue-activity-1-image-0"), "the comment's image is drawn");
    let panel = t.panel.clone();
    cx.update(|window, cx| window.focus(&panel.read(cx).focus.clone()));
    click(cx, "issue-activity-1-image-0");
    assert!(cx.update(|window, cx| ui_rich::viewer::is_image_viewer_open(window, cx)), "the viewer opened");
    assert_eq!(t.closes.get(), 0, "the click did not reach the panel");

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!cx.update(|window, cx| ui_rich::viewer::is_image_viewer_open(window, cx)), "Esc closed the viewer");
    assert_eq!(t.closes.get(), 0, "the panel stays open");
    assert!(cx.update(|window, cx| panel.read(cx).focus.is_focused(window)), "focus came back to the panel");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(t.closes.get(), 1, "the next Esc closes the panel");
}

#[gpui::test]
fn the_descriptions_images_open_in_the_viewer_too(cx: &mut TestAppContext) {
    let image = |name: &str| hub::api::Attachment {
        blob: hub::api::BlobId::new(format!("blob-{name}")),
        name: name.to_string(),
        media_type: "image/png".into(),
        bytes: 1024,
    };
    let (_t, cx) = open_with(cx, "PAY-1427", |w| {
        if let Some(issue) = w.issues.iter_mut().find(|i| i.key == IssueKey::new("PAY-1427")) {
            issue.images = vec![image("before.png"), image("after.png")];
        }
    });
    assert!(shown(cx, "issue-image-0"));
    click(cx, "issue-image-1");
    assert!(cx.update(|window, cx| ui_rich::viewer::is_image_viewer_open(window, cx)));
    let names: Vec<String> = cx.update(|_, cx| {
        let images = [image("before.png"), image("after.png")];
        crate::panel::viewer_images(&images, cx).into_iter().map(|i| i.name.to_string()).collect()
    });
    assert_eq!(names, ["before.png", "after.png"], "each image is shown under its file name, not its blob id");
}
