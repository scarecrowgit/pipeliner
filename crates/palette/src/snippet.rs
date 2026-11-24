//! Matched text: highlight ranges made safe for the text shaper, snippets cut around their
//! first match, and one-line text with the match marked.

use std::ops::Range;

use gpui::{Div, HighlightStyle, SharedString, StyledText, div, prelude::*, px};
use ui::theme::{ACCENT_SOFT, ACCENT_TEXT};

/// Characters of context kept before the first match.
const LEAD: usize = 28;
/// The longest text a row shapes; a snippet is one line, the rest would only cost time.
const MAX_BYTES: usize = 320;
const ELLIPSIS: &str = "…";

/// Highlight ranges sorted, merged where they touch and cut to what is valid for `text`:
/// inside it and on character boundaries. The search index counts bytes of its own copy of
/// the text, so a range that does not fit is dropped rather than trusted.
pub(crate) fn clean(text: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut valid: Vec<Range<usize>> = ranges
        .iter()
        .filter(|r| r.start < r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        .cloned()
        .collect();
    valid.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(valid.len());
    for range in valid {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// A one-line piece of text and the ranges to mark in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snippet {
    pub text: String,
    pub marks: Vec<Range<usize>>,
}

/// `text` as one line that starts shortly before its first match (`…` marks the cut), so the
/// match is on screen when the row truncates the rest. Line breaks become spaces.
pub(crate) fn window(text: &str, ranges: &[Range<usize>]) -> Snippet {
    let marks = clean(text, ranges);
    let first = marks.first().map_or(0, |r| r.start);
    let start = text[..first].char_indices().rev().nth(LEAD).map_or(0, |(at, _)| at);
    let mut end = text.len().min(start + MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = start > 0;
    let shift = if cut { ELLIPSIS.len() } else { 0 };
    let mut out = String::with_capacity(end - start + shift);
    if cut {
        out.push_str(ELLIPSIS);
    }
    // Whitespace characters that end a line are one byte each, as the space that replaces them.
    out.extend(text[start..end].chars().map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c }));
    let marks = marks
        .into_iter()
        .filter(|r| r.end <= end && r.start >= start)
        .map(|r| r.start - start + shift..r.end - start + shift)
        .collect();
    Snippet { text: out, marks }
}

/// The style of a matched word: the accent on its soft ground, as the mockup's `mark`.
fn mark_style() -> HighlightStyle {
    HighlightStyle { color: Some(ACCENT_TEXT), background_color: Some(ACCENT_SOFT), ..HighlightStyle::default() }
}

/// One line of `text`, `height` px tall, that ends in `…` when it does not fit, with `marks`
/// highlighted. The same recipe as the kit's `truncate` (a clamped, wrapping line is
/// re-measured at the final width), for text that has styled parts.
pub(crate) fn line(text: impl Into<SharedString>, marks: &[Range<usize>], height: f32) -> Div {
    let text: SharedString = text.into();
    let marks = clean(&text, marks);
    let styled = StyledText::new(text).with_highlights(marks.into_iter().map(|r| (r, mark_style())));
    div().flex().flex_grow().flex_shrink().min_w_0().h(px(height)).overflow_hidden().child(
        div().w_full().min_w_0().overflow_hidden().whitespace_normal().line_clamp(1).text_ellipsis().child(styled),
    )
}

/// Wrapped text of at most `lines` lines with `marks` highlighted, for the preview.
pub(crate) fn block(text: impl Into<SharedString>, marks: &[Range<usize>], lines: usize) -> Div {
    let text: SharedString = text.into();
    let marks = clean(&text, marks);
    let styled = StyledText::new(text).with_highlights(marks.into_iter().map(|r| (r, mark_style())));
    div().w_full().min_w_0().overflow_hidden().whitespace_normal().line_clamp(lines).text_ellipsis().child(styled)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn clean_sorts_merges_and_drops_what_does_not_fit() {
        let text = "héllo wörld";
        // 'é' is two bytes: 2 is inside it. 100 is past the end. 3..3 is empty.
        let ranges = [7..9, 0..2, 1..3, 2..4, 100..101, 3..3, 7..12];
        let cleaned = clean(text, &ranges);
        assert!(cleaned.iter().all(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end)));
        assert!(cleaned.windows(2).all(|pair| pair[0].end < pair[1].start));
    }

    #[test]
    fn a_match_far_in_comes_into_view_after_an_ellipsis() {
        let text = format!("{} retry {}", "word ".repeat(40), "tail ".repeat(10));
        let at = text.find("retry").unwrap_or(0);
        let snippet = window(&text, &[at..at + 5]);
        assert!(snippet.text.starts_with('…'));
        let mark = &snippet.marks[0];
        assert_eq!(&snippet.text[mark.clone()], "retry");
        assert!(mark.start < 40, "the match is near the start of the line");
    }

    #[test]
    fn a_match_at_the_start_keeps_the_text_whole_and_one_line() {
        let snippet = window("Retry now\nthen stop", &[0..5]);
        assert_eq!(snippet.text, "Retry now then stop");
        assert_eq!(snippet.marks, [0..5]);
    }

    #[test]
    fn long_text_is_cut_and_marks_beyond_the_cut_are_dropped() {
        let text = format!("retry {}", "x".repeat(2000));
        let snippet = window(&text, &[0..5, 1500..1505]);
        assert!(snippet.text.len() <= MAX_BYTES);
        assert_eq!(snippet.marks, [0..5]);
    }

    #[test]
    fn multibyte_text_never_cuts_inside_a_character() {
        let text = "ж".repeat(400);
        let snippet = window(&text, &[]);
        assert!(snippet.text.len() <= MAX_BYTES);
        assert!(snippet.text.chars().all(|c| c == 'ж'));
    }
}
