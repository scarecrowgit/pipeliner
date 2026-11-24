//! The runs each laid-out line is shaped with: a bold label (or inline code, a link) must
//! stay its own font run on every wrapped line, in lists, quotes and table cells, with ASCII
//! and Cyrillic text and multi-byte punctuation (`·`, `→`). GPUI's test platform gives every
//! font the same id, so this replays GPUI's own run merge instead of reading a window.

use gpui::FontWeight;
use ui::text_runs::font_runs_as_shaped;

use super::super::layout::{Look, runs};
use super::super::parse::parse;
use super::super::rich::{SpanStyle, units};

const DOC: &str = "\
- **Triage (Decision):** yes · HAR-4 adds user-visible controls → the reviewer must sign off before the rollout reaches the payments team.
- **Риск (оценка):** средний · ключ хранится в `charges.idempotency_key` → повторы не создают второй платёж, см. [старые записи](https://example.com/b) ниже.

A paragraph after the list: nothing here is bold, and it is long enough to wrap twice.

> **Note (решение):** a quote with a bold label wraps onto a second line → the rest stays regular.

| Check | Result |
|:--|:--|
| **Triage:** yes · HAR-4 adds controls → sign-off needed | **Ок:** проверено · plain text after the label |
";

/// Font (weight, mono) the span at byte `at` asks for.
fn wanted(spans: &[super::super::rich::Span], at: usize, look: &Look) -> (FontWeight, bool) {
    let style: SpanStyle = spans.iter().find(|s| s.range.start <= at && at < s.range.end).map(|s| s.style).unwrap_or_default();
    let weight = if style.bold { FontWeight(look.weight.0.max(FontWeight::SEMIBOLD.0)) } else { look.weight };
    (weight, look.mono || style.code)
}

/// Every split of every unit into two "wrapped lines" (at each char boundary), shaped as
/// GPUI shapes it, gives each byte the font its span asks for.
#[test]
fn every_wrapped_line_keeps_each_span_in_its_own_font() {
    let look = Look::text(ui::theme::BODY, ui::theme::TEXT);
    let doc = parse(DOC);
    let units = units(&doc, &[]);
    assert!(units.len() >= 6, "{} units", units.len());
    for unit in &units {
        let rich = &unit.text;
        let text = &rich.text;
        for (cut, _) in text.char_indices().skip(1) {
            for line in [0..cut, cut..text.len()] {
                let shaped = font_runs_as_shaped(&runs(rich, line.clone(), &look, None));
                assert_eq!(shaped.iter().map(|(_, n)| n).sum::<usize>(), line.len());
                let mut at = line.start;
                for (font, len) in shaped {
                    for b in at..at + len {
                        let mono = font.family.as_ref() == ui::theme::FONT_MONO;
                        assert_eq!(
                            (font.weight, mono),
                            wanted(&rich.spans, b, &look),
                            "byte {b} of {text:?} on the line {line:?}"
                        );
                    }
                    at += len;
                }
            }
        }
    }
}
