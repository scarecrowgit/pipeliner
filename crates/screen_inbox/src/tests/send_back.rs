//! The send-back dialog: the note, the start modes, the variant count and the estimate.

use gpui::{TestAppContext, VisualTestContext};
use hub::InboxFocus;
use hub::api::{ReviewDecision, ReviewId, StartFrom};

use super::{Open, REVIEW, calls, click, open, shown};
use crate::send_back::EstimateState;

fn open_dialog(cx: &mut TestAppContext) -> (Open, &mut VisualTestContext) {
    let (t, cx) = open(cx, Some(InboxFocus::Review(ReviewId::new(REVIEW))));
    click(cx, "review-send-back");
    assert!(shown(cx, "send-back-note"));
    (t, cx)
}

/// The estimate the dialog shows, as (seconds, cents).
fn estimate(t: &Open, cx: &mut VisualTestContext) -> Option<(u64, i64)> {
    cx.run_until_parked();
    t.view
        .read_with(cx, |v, cx| match &v.send_back.as_ref()?.read(cx).estimate {
            EstimateState::Ready(e) => Some((e.secs, (e.cost_usd * 100.).round() as i64)),
            _ => None,
        })
}

fn dialog_open(cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| ui::overlays(window, cx).is_open(crate::send_back::DIALOG_ID, cx))
}

fn note(t: &Open, cx: &mut VisualTestContext) -> String {
    t.view.read_with(cx, |v, cx| {
        v.send_back
            .as_ref()
            .map(|d| d.read(cx).note.read(cx).text().to_string())
            .unwrap_or_default()
    })
}

fn sent(text: &str, start: StartFrom, variants: u32) -> String {
    format!(
        "{REVIEW} {:?}",
        ReviewDecision::SendBack {
            note: text.to_string(),
            start,
            variants
        }
    )
}

#[gpui::test]
async fn typing_reaches_the_note_through_the_window(cx: &mut TestAppContext) {
    let (t, cx) = open_dialog(cx);
    cx.simulate_input("Keep A");
    cx.run_until_parked();
    assert_eq!(note(&t, cx), "Keep A");
}

#[gpui::test]
async fn sending_from_the_picked_variant_with_more_variants(cx: &mut TestAppContext) {
    let (t, cx) = open_dialog(cx);
    // Fixture: the mean of the variants' time and cost, 0.6 of it for one variant's branch.
    assert_eq!(estimate(&t, cx), Some((60, 25)));
    cx.simulate_input("Keep A");
    click(cx, "send-back-stepper-inc");
    assert_eq!(
        estimate(&t, cx),
        Some((60, 38)),
        "the estimate follows the count"
    );
    click(cx, "send-back-confirm");
    assert_eq!(
        calls(&t.api, "decide_review"),
        [sent("Keep A", StartFrom::Variant { index: 1 }, 3)]
    );
    assert!(!dialog_open(cx));
}

#[gpui::test]
async fn sending_all_variants_back_builds_one_per_variant(cx: &mut TestAppContext) {
    let (t, cx) = open_dialog(cx);
    click(cx, "send-back-from-all");
    assert_eq!(estimate(&t, cx), Some((100, 63)));
    click(cx, "send-back-note");
    cx.simulate_input("Add a webhook test");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(
        calls(&t.api, "decide_review"),
        [sent("Add a webhook test", StartFrom::All, 3)]
    );
}

#[gpui::test]
async fn starting_over_keeps_the_chosen_count(cx: &mut TestAppContext) {
    let (t, cx) = open_dialog(cx);
    click(cx, "send-back-from-over");
    click(cx, "send-back-stepper-dec");
    assert_eq!(estimate(&t, cx), Some((100, 21)));
    click(cx, "send-back-note");
    cx.simulate_input("Try a queue");
    click(cx, "send-back-confirm");
    assert_eq!(
        calls(&t.api, "decide_review"),
        [sent("Try a queue", StartFrom::Over, 1)]
    );
}

#[gpui::test]
async fn without_a_note_nothing_is_sent_and_escape_closes(cx: &mut TestAppContext) {
    let (t, cx) = open_dialog(cx);
    click(cx, "send-back-confirm");
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert!(calls(&t.api, "decide_review").is_empty());
    assert!(dialog_open(cx));
    click(cx, "send-back-note");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!dialog_open(cx), "Escape in the note closes the dialog");
    assert!(calls(&t.api, "decide_review").is_empty());
}

#[gpui::test]
async fn a_stale_estimate_is_dropped(cx: &mut TestAppContext) {
    let (t, cx) = open_dialog(cx);
    // Two changes before any answer lands: only the last one's answer counts.
    let dialog = t
        .view
        .read_with(cx, |v, _| v.send_back.clone())
        .expect("the dialog is open");
    dialog.update(cx, |d, cx| {
        d.set_mode(crate::send_back::Mode::All, cx);
        d.set_mode(crate::send_back::Mode::Over, cx);
    });
    assert_eq!(estimate(&t, cx), Some((100, 42)));
}
