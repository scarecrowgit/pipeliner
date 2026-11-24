//! The comment composer: a mention completed and sent with Cmd+Enter through the window's key
//! dispatch, images stored before they go out, and the draft kept per issue.

use std::sync::Arc;

use gpui::{AppContext as _, Image, ImageFormat, TestAppContext};
use hub::api::{IssueEventKind, RunnableRef, SessionId};
use hub::fixtures::FIXER_1427;
use hub::model::IssueKey;
use ui_rich::Attachment;

use super::{calls, focus_composer, open, text};
use crate::IssuePanel;

/// A 4 × 3 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x04, 0x00, 0x00, 0x00, 0x03, 0x08, 0x02, 0x00, 0x00, 0x00, 0x3b, 0x96, 0x39, 0x91, 0x00, 0x00, 0x00, 0x10, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x88, 0xcb, 0xba, 0x04, 0x47, 0x0c, 0x38, 0x39, 0x00, 0x71, 0x8a, 0x13, 0x39,
    0x22, 0x16, 0x30, 0x2b, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn last_comment(t: &super::Open) -> Option<(String, Vec<RunnableRef>, usize)> {
    t.api.with_world(|w| {
        w.issue_events[&IssueKey::new("PAY-1427")].iter().rev().find_map(|e| match &e.kind {
            IssueEventKind::Comment { text, mentions, images } => Some((text.clone(), mentions.clone(), images.len())),
            _ => None,
        })
    })
}

#[gpui::test]
fn a_mention_is_completed_and_the_comment_goes_out_with_cmd_enter(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    focus_composer(&t, cx);
    cx.simulate_input("Try again @Fix");
    cx.run_until_parked();
    assert!(t.composer.read_with(cx, |c, _| c.menu_open()), "the @ menu is open");
    let hint = t.composer.read_with(cx, |c, _| c.selected());
    assert_eq!(hint, Some(0));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text(&t, cx), "Try again @Fixer ");
    cx.simulate_input("with batches of 50");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();

    assert_eq!(calls(&t.api, "comment"), ["PAY-1427 Try again @Fixer with batches of 50"]);
    assert_eq!(text(&t, cx), "", "the box is cleared");
    let (said, mentions, _) = last_comment(&t).unwrap_or_default();
    assert_eq!(said, "Try again @Fixer with batches of 50");
    let fixer_step = RunnableRef::session(&SessionId::new(FIXER_1427));
    assert_eq!(mentions, [fixer_step], "the run's Fixer is mentioned, not the library's");
    assert!(super::shown(cx, "issue-activity-12"), "the new comment is in the feed");
}

#[gpui::test]
fn the_at_menu_offers_the_runs_agents_first(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    let rows = |busy: bool, cx: &mut gpui::VisualTestContext| {
        t.panel.read_with(cx, |p, cx| {
            let mut agents = p.run_agents(cx);
            for agent in &mut agents {
                agent.busy |= busy;
            }
            let (rows, total) = super::super::comment::mention_rows(p.catalog.read(cx), &agents, "fix", None, "PAY-1427");
            let rows: Vec<_> = rows.into_iter().map(|r| (r.label.to_string(), r.detail.map(|d| d.to_string()), r.hint.map(|h| h.to_string()))).collect();
            (rows, total)
        })
    };
    let (shown, total) = rows(false, cx);
    assert_eq!(shown[0].0, "Fixer");
    assert_eq!(shown[0].1.as_deref(), Some("Agent · Fixer step of run 2 · continues in its worktree"));
    assert_eq!(shown[0].2.as_deref(), Some("run 2"));
    assert_eq!(shown.iter().filter(|(name, _, _)| name == "Fixer").count(), 1, "the library's Fixer gives way: {shown:#?}");
    assert_eq!(total, shown.len());
    let (shown, _) = rows(true, cx);
    assert_eq!(shown[0].1.as_deref(), Some("Agent · Fixer step of run 2 · running now"));
}

#[gpui::test]
fn the_at_menu_offers_agents_orchestrators_and_pipelines(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    let rows = t.panel.read_with(cx, |p, cx| {
        let (rows, total) = super::super::comment::mention_rows(p.catalog.read(cx), &[], "re", None, "PAY-1427");
        (rows.into_iter().map(|r| (r.label.to_string(), r.detail.map(|d| d.to_string()))).collect::<Vec<_>>(), total)
    });
    let (rows, total) = rows;
    assert_eq!(rows[0].0, "Release captain", "names starting with the query come first, orchestrators first");
    assert_eq!(rows[0].1.as_deref(), Some("Orchestrator · starts a new run on PAY-1427"));
    assert!(rows.iter().any(|(name, _)| name == "Reproducer"));
    assert!(total >= 2);
    let attached = t.panel.read_with(cx, |p, cx| {
        let pipeline = RunnableRef::pipeline("pay-triage");
        let (rows, _) = super::super::comment::mention_rows(p.catalog.read(cx), &[], "pay", Some(&pipeline), "PAY-1427");
        rows.first().and_then(|r| r.detail.clone()).map(|d| d.to_string())
    });
    assert_eq!(attached.as_deref(), Some("Pipeline · on this issue now · gets your comment as input"));
}

#[gpui::test]
fn a_pasted_image_is_stored_and_goes_out_with_the_comment(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    focus_composer(&t, cx);
    cx.simulate_input("Trace attached");
    let image = Arc::new(Image::from_bytes(ImageFormat::Png, PNG.to_vec()));
    let added = t.composer.update(cx, |c, cx| c.add_attachment(Attachment::image("trace.png", image), cx));
    assert!(added.is_ok());
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "put_image"), ["trace.png"]);
    assert!(t.composer.read_with(cx, |c, _| c.can_send()), "the upload finished");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    let (said, _, images) = last_comment(&t).unwrap_or_default();
    assert_eq!(said, "Trace attached");
    assert_eq!(images, 1, "the image went out with the comment");
}

#[gpui::test]
fn the_draft_survives_building_the_panel_again(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "PAY-1427");
    focus_composer(&t, cx);
    cx.simulate_input("half a thought");
    cx.run_until_parked();
    let again = cx.update(|window, cx| cx.new(|cx| IssuePanel::new(IssueKey::new("PAY-1427"), window, cx)));
    let restored = again.read_with(cx, |p, cx| p.composer().read(cx).text().to_string());
    assert_eq!(restored, "half a thought");
    let other = cx.update(|window, cx| cx.new(|cx| IssuePanel::new(IssueKey::new("PAY-1440"), window, cx)));
    assert_eq!(other.read_with(cx, |p, cx| p.composer().read(cx).text().to_string()), "");
}
