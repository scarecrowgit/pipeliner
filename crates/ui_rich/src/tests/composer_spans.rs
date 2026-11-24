//! The composer's GPUI-free bookkeeping: mention spans along edits, limits and wording.

use crate::composer::spans::{changed_span, mention_at, reconcile, summary};
use crate::{Attachment, AttachmentLimits, Mention, TriggerKind};

fn mention(text: &str, start: usize) -> Mention {
    Mention {
        kind: TriggerKind::Mention,
        id: text.to_string().into(),
        text: text.to_string().into(),
        range: start..start + text.len(),
    }
}

#[test]
fn the_changed_span_is_found_between_prefix_and_suffix() {
    assert_eq!(changed_span("abc", "abXc"), (2..2, 1));
    assert_eq!(changed_span("abc", "ac"), (1..2, 0));
    assert_eq!(changed_span("héllo", "hallo"), (1..3, 1));
    assert_eq!(changed_span("aa", "aaa"), (2..2, 1));
    assert_eq!(changed_span("", "x"), (0..0, 1));
}

#[test]
fn spans_before_stay_after_shift_and_touched_go() {
    let old = "hi @Fixer and @Codex";
    let mut m = vec![mention("@Fixer", 3), mention("@Codex", 14)];
    reconcile(old, "hi @Fixer and you @Codex", &mut m);
    assert_eq!(m[0].range, 3..9);
    assert_eq!(m[1].range, 18..24);
    let mut m = vec![mention("@Fixer", 3), mention("@Codex", 14)];
    reconcile(old, "hi @Fixr and @Codex", &mut m);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].range, 13..19);
}

#[test]
fn a_span_that_runs_into_a_word_is_dropped() {
    let mut m = vec![mention("@Fixer", 0)];
    reconcile("@Fixer ", "@Fixers ", &mut m);
    assert!(m.is_empty());
    // Typing before it (on the way to a space) keeps it.
    let mut m = vec![mention("@Fixer", 0)];
    reconcile("@Fixer", "h@Fixer", &mut m);
    assert_eq!(m[0].range, 1..7);
}

#[test]
fn deletes_find_the_mention_on_their_side() {
    let m = vec![mention("@Fixer", 4)];
    assert_eq!(mention_at(&m, 10, true), Some(4..10));
    assert_eq!(mention_at(&m, 4, true), None);
    assert_eq!(mention_at(&m, 4, false), Some(4..10));
    assert_eq!(mention_at(&m, 10, false), None);
    assert_eq!(mention_at(&m, 7, true), Some(4..10));
}

#[test]
fn the_limits_note_counts_and_names_the_limit() {
    let img = Attachment::image("a.png", std::sync::Arc::new(gpui::Image::empty()));
    let up = img.clone().uploading(0.62);
    let limits = AttachmentLimits::default();
    assert_eq!(summary(&[img.clone(), up], limits), ["2 of 10 images", "1 uploading", "20 MB max each"]);
    let file = Attachment::file("a.txt", "/tmp/a.txt".into(), 1);
    assert_eq!(summary(&[img, file], limits)[0], "2 of 10 attachments");
}
