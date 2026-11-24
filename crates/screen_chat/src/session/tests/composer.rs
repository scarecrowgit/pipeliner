//! Sending, stopping, queueing, drafts, completions and disabled composers.

use std::sync::Arc;

use gpui::{AppContext as _, Image, ImageFormat, TestAppContext};
use hub::api::ContentBlock;
use hub::model::SessionId;
use ui_rich::Attachment;

use super::{FIX, calls, click, focus_composer, open, open_with, text, type_text};
use crate::SessionView;

/// A 4 × 3 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x04, 0x00, 0x00, 0x00, 0x03, 0x08, 0x02, 0x00, 0x00, 0x00, 0x3b, 0x96, 0x39, 0x91, 0x00, 0x00, 0x00, 0x10, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x88, 0xcb, 0xba, 0x04, 0x47, 0x0c, 0x38, 0x39, 0x00, 0x71, 0x8a, 0x13, 0x39,
    0x22, 0x16, 0x30, 0x2b, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[gpui::test]
fn submit_sends_the_text_with_the_uploaded_image_and_clears_the_composer(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    type_text(&t, "Run the charges suite again", cx);
    let image = Arc::new(Image::from_bytes(ImageFormat::Png, PNG.to_vec()));
    let added = t.composer.update(cx, |c, cx| c.add_attachment(Attachment::image("shot.png", image), cx));
    assert!(added.is_ok());
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "put_image"), ["shot.png"]);
    assert!(t.composer.read_with(cx, |c, _| c.can_send()), "the upload finished");

    click(cx, "chat-send");
    assert_eq!(calls(&t.api, "send"), [format!("{FIX} Run the charges suite again")]);
    assert_eq!(text(&t, cx), "");
    assert!(t.composer.read_with(cx, |c, _| c.attachments().is_empty()));
    let images = t.api.with_world(|w| {
        w.messages[&SessionId::new(FIX)]
            .iter()
            .flat_map(|m| &m.message.content)
            .filter(|b| matches!(b, ContentBlock::Image { name, .. } if name == "shot.png"))
            .count()
    });
    assert_eq!(images, 1, "the image went out with the message");
}

#[gpui::test]
fn a_busy_session_queues_what_is_sent_and_stop_interrupts(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, "chat-refund-timeout");
    assert!(t.composer.read_with(cx, |c, _| c.is_busy()));
    assert!(super::shown(cx, "chat-stop"));

    type_text(&t, "Also check the batcher", cx);
    focus_composer(&t, cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "send"), ["chat-refund-timeout Also check the batcher"]);
    assert_eq!(text(&t, cx), "", "a queued message leaves the composer");
    let queued = t.view.read_with(cx, |v, cx| v.model.read(cx).queued().len());
    assert_eq!(queued, 1, "the queue is read again after sending");

    click(cx, "chat-stop");
    assert_eq!(calls(&t.api, "interrupt"), ["chat-refund-timeout"]);
}

#[gpui::test]
fn the_draft_survives_building_the_view_again(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    type_text(&t, "half a thought", cx);
    cx.run_until_parked();
    let again = cx.update(|window, cx| cx.new(|cx| SessionView::new(SessionId::new(FIX), window, cx)));
    let restored = again.read_with(cx, |v, cx| v.composer().read(cx).text().to_string());
    assert_eq!(restored, "half a thought");
    let other = cx.update(|window, cx| cx.new(|cx| SessionView::new(SessionId::new("chat-ledger"), window, cx)));
    assert_eq!(other.read_with(cx, |v, cx| v.composer().read(cx).text().to_string()), "");
}

#[gpui::test]
fn at_and_slash_completions_arrive_and_stale_ones_are_dropped(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, FIX);
    type_text(&t, "@retry", cx);
    cx.run_until_parked();
    assert!(t.composer.read_with(cx, |c, _| c.menu_open()));
    t.composer.update(cx, |c, cx| c.accept(0, cx));
    assert_eq!(text(&t, cx), "@src/charges/retry.ts ");

    t.composer.update(cx, |c, cx| c.clear(cx));
    type_text(&t, "@Payments", cx);
    cx.run_until_parked();
    t.composer.update(cx, |c, cx| c.accept(0, cx));
    assert_eq!(text(&t, cx), "@Payments-lead ");

    t.composer.update(cx, |c, cx| c.clear(cx));
    type_text(&t, "/comp", cx);
    cx.run_until_parked();
    t.composer.update(cx, |c, cx| c.accept(0, cx));
    assert_eq!(text(&t, cx), "/compact ");

    t.composer.update(cx, |c, cx| c.clear(cx));
    cx.run_until_parked();
    let before = t.view.read_with(cx, |v, _| v.input.applied);
    type_text(&t, "@ret", cx);
    type_text(&t, "ry.te", cx);
    cx.run_until_parked();
    assert_eq!(t.view.read_with(cx, |v, _| v.input.applied), before + 1, "only the newest answer is applied");
    t.composer.update(cx, |c, cx| c.accept(0, cx));
    assert_eq!(text(&t, cx), "@src/charges/retry.test.ts ");
}

#[gpui::test]
fn archived_and_sub_agent_sessions_take_no_input(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, FIX, |w| {
        if let Ok(s) = w.session_mut(&SessionId::new(FIX)) {
            s.archived = true;
        }
    });
    let reason = t.composer.read_with(cx, |c, _| c.disabled_reason().map(|r| r.to_string()));
    assert_eq!(reason.as_deref(), Some("Archived chat · restore it to reply"));

    let child = cx.update(|window, cx| cx.new(|cx| SessionView::new(SessionId::new("chat-1423-explore"), window, cx)));
    cx.run_until_parked();
    let reason = child.read_with(cx, |v, cx| v.composer().read(cx).disabled_reason().map(|r| r.to_string()));
    assert_eq!(reason.as_deref(), Some("Sub-agent session · read-only"));
}
