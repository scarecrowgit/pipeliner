//! The review detail: variants, files, diffs, decisions, closed reviews.

use gpui::TestAppContext;
use hub::InboxFocus;
use hub::api::{ReviewId, ReviewNote, ReviewState};

use super::{REVIEW, calls, click, keys, open, open_with, shown};

fn item(id: &str) -> Option<InboxFocus> {
    Some(InboxFocus::Item(hub::api::InboxId::new(id)))
}

fn review(id: &str) -> Option<InboxFocus> {
    Some(InboxFocus::Review(ReviewId::new(id)))
}

#[gpui::test]
async fn the_first_variant_and_its_first_file_open_with_the_diff(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, review(REVIEW));
    let ready = t.view.read_with(cx, |v, cx| {
        let open = v.review.as_ref()?;
        Some((
            open.variant,
            open.model
                .read(cx)
                .diff(1, "src/charges/retry.ts")?
                .is_ready(),
        ))
    });
    assert_eq!(ready, Some((Some(1), true)));
    assert!(shown(cx, "review-table"));
    assert!(shown(cx, "review-diff"));
}

#[gpui::test]
async fn choosing_a_variant_and_a_file_loads_that_diff(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, review(REVIEW));
    click(cx, "review-variant-2");
    click(cx, "review-file-1");
    let state = t.view.read_with(cx, |v, cx| {
        let open = v.review.as_ref()?;
        Some((
            open.variant,
            open.file,
            open.model.read(cx).diff(2, "src/db/locks.ts")?.is_ready(),
        ))
    });
    assert_eq!(state, Some((Some(2), 1, true)));
    assert!(shown(cx, "review-diff"));
}

#[gpui::test]
async fn ship_records_the_chosen_variant_and_closes_the_review(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, review(REVIEW));
    click(cx, "review-variant-3");
    click(cx, "review-ship");
    assert_eq!(
        calls(&t.api, "decide_review"),
        ["review-1423 Ship { variant: 3 }"]
    );
    // The decided item leaves the queue: the next row opens.
    assert_eq!(super::focus(&t, cx), item("inbox-1435"));
    // The decided review is read-only in the archive.
    click(cx, "inbox-tabs-2");
    assert_eq!(super::focus(&t, cx), item("inbox-1423"));
    let state = t
        .view
        .read_with(cx, |v, cx| v.open_review(cx).map(|r| r.state));
    assert_eq!(state, Some(ReviewState::Shipped));
    assert!(shown(cx, "review-closed"));
}

#[gpui::test]
async fn deciding_opens_the_next_row(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, None);
    click(cx, "inbox-row-2");
    click(cx, "review-skip");
    assert_eq!(calls(&t.api, "decide_review"), ["review-1427 Skip"]);
    assert_eq!(
        super::rows(&t, cx),
        ["inbox-1423", "inbox-1435", "inbox-1431"]
    );
    assert_eq!(
        super::focus(&t, cx),
        item("inbox-1431"),
        "the row now at its place"
    );
    assert_eq!(super::open_review(&t, cx).as_deref(), Some("review-1431"));
}

#[gpui::test]
async fn skip_records_a_skip(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, review("review-1427"));
    click(cx, "review-skip");
    assert_eq!(calls(&t.api, "decide_review"), ["review-1427 Skip"]);
}

#[gpui::test]
async fn a_closed_review_is_read_only(cx: &mut TestAppContext) {
    let (t, cx) = open_with(cx, review(REVIEW), |world| {
        let r = &mut world.reviews[0];
        r.state = ReviewState::Shipped;
        r.picked = Some(2);
        r.feedback.push(ReviewNote {
            at: super::NOW - 60_000,
            text: "Keep the public signature".to_string(),
        });
    });
    assert!(shown(cx, "review-closed"));
    assert!(shown(cx, "review-notes"));
    for button in ["review-ship", "review-skip", "review-send-back"] {
        assert!(!shown(cx, button), "{button} is drawn");
    }
    let label = t.view.read_with(cx, |v, cx| {
        v.open_review(cx).map(|r| crate::review::decided_label(&r))
    });
    assert_eq!(
        label.as_deref(),
        Some("Shipped variant B · Lock the row before charging")
    );
    keys(cx, "r");
    let open =
        cx.update(|window, cx| ui::overlays(window, cx).is_open(crate::send_back::DIALOG_ID, cx));
    assert!(!open, "R does nothing on a closed review");
    assert!(calls(&t.api, "decide_review").is_empty());
}

#[gpui::test]
async fn a_review_without_an_item_opens_the_first_row(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, review("review-gone"));
    assert_eq!(super::focus(&t, cx), item("inbox-1423"));
}

#[test]
fn test_results_read_as_in_the_mockup() {
    use hub::api::{TestResult, Variant};
    let base = Variant {
        index: 1,
        name: String::new(),
        approach: String::new(),
        branch: None,
        worktree: None,
        session: None,
        added: 0,
        removed: 0,
        files: Vec::new(),
        tests: Some(TestResult {
            passed: 212,
            total: 212,
        }),
        tests_ok: None,
        secs: 0,
        cost_usd: 0.0,
        renders: Vec::new(),
    };
    let label = |v: &Variant| crate::review::tests_label(v).0;
    assert_eq!(label(&base), "212 of 212 passed");
    let failed = Variant {
        tests: Some(TestResult {
            passed: 211,
            total: 212,
        }),
        ..base.clone()
    };
    assert_eq!(label(&failed), "211 of 212 · 1 failed");
    let none = Variant {
        tests: Some(TestResult {
            passed: 0,
            total: 0,
        }),
        ..base.clone()
    };
    assert_eq!(label(&none), "No tests");
    let only_ok = Variant {
        tests: None,
        tests_ok: Some(false),
        ..base.clone()
    };
    assert_eq!(label(&only_ok), "Failed");
    let not_run = Variant {
        tests: None,
        tests_ok: None,
        ..base
    };
    assert_eq!(label(&not_run), "Not run");
}

#[gpui::test]
async fn the_action_bar_summary_gives_way_to_the_buttons(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx, review(REVIEW));
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(900.)));
    cx.run_until_parked();
    cx.run_until_parked();
    let summary = cx
        .debug_bounds("review-summary-tests")
        .expect("the tests are drawn");
    let send = cx
        .debug_bounds("review-send-back")
        .expect("the button is drawn");
    assert!(
        summary.right() <= send.left(),
        "the summary ends at {:?}, the button starts at {:?}",
        summary.right(),
        send.left()
    );
}
