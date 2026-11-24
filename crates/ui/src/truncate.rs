//! One-line text that does not fit: styled text that ends in `…` ([`truncate_styled`]), and
//! paths that keep their head and tail and lose the middle ([`truncate_middle`]). Both are
//! cut at the width they are given, at prepaint.
//!
//! Both follow the recipe of [`crate::layout::truncate`]: the cell fills the rest of its row
//! and may shrink below its content; in a shrink-to-fit parent it is as wide as the text.

use std::ops::Range;

use gpui::{
    App, Bounds, Div, Element, ElementId, GlobalElementId, HighlightStyle, InspectorElementId,
    IntoElement, LayoutId, Pixels, ShapedLine, SharedString, Style, StyledText, TextStyle, Window,
    div, prelude::*, px,
};

/// What stands for the text a cut leaves out.
pub const ELLIPSIS: &str = "…";

/// `highlights` made safe for `text`: inside it, on character boundaries, sorted, and with
/// overlaps cut so each byte has at most one style (the earlier range keeps the overlap).
/// Empty ranges are dropped. GPUI panics on ranges that break these rules.
pub fn clean_highlights(
    text: &str,
    highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut valid: Vec<(Range<usize>, HighlightStyle)> = highlights
        .into_iter()
        .filter(|(r, _)| {
            r.start < r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)
        })
        .collect();
    valid.sort_by_key(|(r, _)| r.start);
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(valid.len());
    for (mut range, style) in valid {
        if let Some((last, _)) = out.last() {
            range.start = range.start.max(last.end);
        }
        if range.start < range.end {
            out.push((range, style));
        }
    }
    out
}

/// [`truncate_lh`](crate::layout::truncate_lh) for text with styled parts: one line of `text`,
/// `line_height` px tall, with each range in `highlights` drawn in its own colour, weight or
/// background, ending in `…` when it does not fit. Bad ranges are dropped
/// ([`clean_highlights`]).
///
/// GPUI's own ellipsis (`text_ellipsis`) drops every styled run after the first once a
/// narrow measure has cut the text, so this cuts the line itself at the width it is given.
pub fn truncate_styled(
    text: impl Into<SharedString>,
    highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    line_height: f32,
) -> Div {
    let text = text.into();
    let highlights = clean_highlights(&text, highlights);
    cell(line_height).child(LineText { text, highlights, cut: Cut::End })
}

/// [`truncate_block`](crate::layout::truncate_block) for text with styled parts: full width,
/// no flex growth, for a column.
pub fn truncate_styled_block(
    text: impl Into<SharedString>,
    highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    line_height: f32,
) -> Div {
    let text = text.into();
    let highlights = clean_highlights(&text, highlights);
    div()
        .flex()
        .w_full()
        .min_w_0()
        .flex_none()
        .h(px(line_height))
        .overflow_hidden()
        .child(LineText { text, highlights, cut: Cut::End })
}

/// Wrapped text with styled parts, at most `lines` lines; what does not fit is cut after the
/// last line, without an ellipsis (GPUI's ellipsis loses styled runs, see
/// [`truncate_styled`]).
pub fn clamp_styled(
    text: impl Into<SharedString>,
    highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    lines: usize,
) -> Div {
    let text: SharedString = text.into();
    let highlights = clean_highlights(&text, highlights);
    div()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .whitespace_normal()
        .line_clamp(lines.max(1))
        .child(StyledText::new(text).with_highlights(highlights))
}

/// One line of `text` that keeps its head and its tail and drops the middle to fit the width
/// it is given: `~/…/payments-api/fix-PAY-1423`. A path (text with `/`) keeps its first
/// segment and as many whole trailing segments as fit; when not even the last one fits, or
/// for other text, characters go from the middle. `line_height` px tall; colour, font and
/// size are inherited.
pub fn truncate_middle(text: impl Into<SharedString>, line_height: f32) -> Div {
    cell(line_height).child(LineText { text: text.into(), highlights: Vec::new(), cut: Cut::Middle })
}

/// The cell of a one-line text in a row: fills the rest, may shrink below its content.
fn cell(line_height: f32) -> Div {
    div().flex().flex_grow().flex_shrink().min_w_0().h(px(line_height)).overflow_hidden()
}

/// `text` cut in the middle until `fits` accepts it (see [`truncate_middle`]). `fits` must
/// grow monotonic with the text: if a string fits, a shorter cut of it fits too. Returns the
/// text whole when it fits, and `…` alone when nothing does.
pub fn middle_cut(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    if fits(text) {
        return text.to_string();
    }
    if let Some(cut) = path_cut(text, &mut fits) {
        return cut;
    }
    char_cut(text, &mut fits)
}

/// The longest `head/…/tail` with whole trailing segments that fits, if there is one.
fn path_cut(text: &str, fits: &mut impl FnMut(&str) -> bool) -> Option<String> {
    // Head: up to and including the first `/` after the first character ("~/", "/", "a/").
    let head_end = text.char_indices().skip(1).find(|(_, c)| *c == '/').map(|(i, _)| i + 1)
        .or_else(|| text.starts_with('/').then_some(1))?;
    let head = &text[..head_end];
    // Each `/` after the head starts a candidate tail, longest first.
    let starts: Vec<usize> = text[head_end..]
        .char_indices()
        .filter(|(_, c)| *c == '/')
        .map(|(i, _)| head_end + i + 1)
        .filter(|&s| s < text.len())
        .collect();
    starts.into_iter().find_map(|s| {
        let candidate = format!("{head}{ELLIPSIS}/{}", &text[s..]);
        fits(&candidate).then_some(candidate)
    })
}

/// `text` cut at the end until `fits` accepts it: the longest `head…` (in characters) that
/// fits, `…` alone when nothing does. Returns the byte length of the head kept, and the
/// text whole (`None`) when it fits.
pub fn end_cut(text: &str, mut fits: impl FnMut(&str) -> bool) -> Option<usize> {
    if fits(text) {
        return None;
    }
    let ends: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let make = |keep: usize| format!("{}{ELLIPSIS}", &text[..ends[keep]]);
    // ends[0] is 0: keeping no characters leaves the ellipsis alone.
    let (mut lo, mut hi) = (0usize, ends.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&make(mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(ends.get(lo).copied().unwrap_or(0))
}

/// The longest `head…tail` made of characters that fits; the tail gets the odd character.
fn char_cut(text: &str, fits: &mut impl FnMut(&str) -> bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let make = |keep: usize| -> String {
        let head = keep / 2;
        let tail = keep - head;
        let mut s: String = chars[..head].iter().collect();
        s.push_str(ELLIPSIS);
        s.extend(&chars[chars.len() - tail..]);
        s
    };
    // Binary search on the number of characters kept.
    let (mut lo, mut hi) = (0usize, chars.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&make(mid)) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    make(lo)
}

/// Where [`LineText`] cuts a line that does not fit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// Keep the start: `Retry the charge…`.
    End,
    /// Keep the head and the tail: `~/…/fix-PAY-1423`.
    Middle,
}

/// One line of text, as wide as its content, shrinking to the width it is given and cut to
/// fit at prepaint (so styled runs survive the cut).
struct LineText {
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    cut: Cut,
}

impl IntoElement for LineText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Text runs for `text` in `style` with `highlights` (clean, in `text`'s offsets).
fn runs(text: &str, style: &TextStyle, highlights: &[(Range<usize>, HighlightStyle)]) -> Vec<gpui::TextRun> {
    let mut out = Vec::with_capacity(highlights.len() * 2 + 1);
    let mut at = 0;
    for (range, h) in highlights {
        let range = range.start.min(text.len())..range.end.min(text.len());
        if range.start >= range.end {
            continue;
        }
        if at < range.start {
            out.push(style.to_run(range.start - at));
        }
        out.push(style.clone().highlight(*h).to_run(range.len()));
        at = range.end;
    }
    if at < text.len() || out.is_empty() {
        out.push(style.to_run(text.len() - at));
    }
    out
}

fn shape(text: &str, style: &TextStyle, highlights: &[(Range<usize>, HighlightStyle)], window: &Window) -> ShapedLine {
    let font_size = style.font_size.to_pixels(window.rem_size());
    let runs = runs(text, style, highlights);
    window.text_system().shape_line(SharedString::from(text.to_string()), font_size, &runs, None)
}

impl Element for LineText {
    type RequestLayoutState = ();
    type PrepaintState = Option<ShapedLine>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let (text, highlights) = (self.text.clone(), self.highlights.clone());
        let mut layout = Style { flex_grow: 1., flex_shrink: 1., ..Style::default() };
        layout.min_size.width = px(0.).into();
        layout.size.height = gpui::relative(1.).into();
        // As wide as the whole text; the parent's `min_w_0` lets it shrink below that.
        let id = window.request_measured_layout(layout, move |known, _, window, _| {
            let width = known.width.unwrap_or_else(|| shape(&text, &style, &highlights, window).width);
            gpui::size(width, known.height.unwrap_or(px(0.)))
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Option<ShapedLine> {
        let style = window.text_style();
        let room = bounds.size.width + px(0.5);
        let line = match self.cut {
            Cut::Middle => {
                let shown = middle_cut(&self.text, |s| shape(s, &style, &[], window).width <= room);
                shape(&shown, &style, &[], window)
            }
            Cut::End => {
                let hl = &self.highlights;
                let kept = end_cut(&self.text, |s| shape(s, &style, hl, window).width <= room);
                match kept {
                    None => shape(&self.text, &style, hl, window),
                    // The ellipsis takes the style of the character before it.
                    Some(head) => {
                        let shown = format!("{}{ELLIPSIS}", &self.text[..head]);
                        let hl: Vec<_> = hl
                            .iter()
                            .filter(|(r, _)| r.start < head)
                            .map(|(r, h)| (r.start..if r.end >= head { shown.len() } else { r.end }, *h))
                            .collect();
                        shape(&shown, &style, &hl, window)
                    }
                }
            }
        };
        Some(line)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        line: &mut Option<ShapedLine>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(line) = line {
            // A paint error means a glyph could not be rasterised; nothing to recover.
            let _ = line.paint_background(bounds.origin, bounds.size.height, window, cx);
            let _ = line.paint(bounds.origin, bounds.size.height, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_chars(max: usize) -> impl FnMut(&str) -> bool {
        move |s: &str| s.chars().count() <= max
    }

    #[test]
    fn text_that_fits_is_kept_whole() {
        assert_eq!(middle_cut("payments-api", by_chars(20)), "payments-api");
    }

    #[test]
    fn a_path_keeps_its_head_and_whole_trailing_segments() {
        let path = "~/code/acme/services/payments-api/fix-PAY-1423";
        assert_eq!(middle_cut(path, by_chars(30)), "~/…/payments-api/fix-PAY-1423");
        assert_eq!(middle_cut(path, by_chars(40)), "~/…/services/payments-api/fix-PAY-1423");
        assert_eq!(middle_cut("/usr/local/share/harness/logs", by_chars(14)), "/usr/…/logs");
    }

    #[test]
    fn a_path_whose_last_segment_is_too_long_is_cut_by_characters() {
        let path = "~/code/a-very-long-worktree-name-for-a-fix";
        let cut = middle_cut(path, by_chars(12));
        assert_eq!(cut.chars().count(), 12);
        assert!(cut.starts_with("~/co") && cut.ends_with("-fix") && cut.contains('…'), "{cut}");
    }

    #[test]
    fn plain_text_loses_its_middle_and_the_tail_gets_the_odd_character() {
        assert_eq!(middle_cut("abcdefghij", by_chars(6)), "ab…hij");
        assert_eq!(middle_cut("abcdefghij", by_chars(1)), "…");
        assert_eq!(middle_cut("abcdefghij", by_chars(0)), "…");
    }

    #[test]
    fn multibyte_text_is_cut_on_characters() {
        let cut = middle_cut("жжжжжжжжжжmмммм", by_chars(7));
        assert_eq!(cut.chars().count(), 7);
        assert!(cut.contains('…'));
    }

    #[test]
    fn an_end_cut_keeps_the_longest_head_that_fits() {
        assert_eq!(end_cut("retry", by_chars(10)), None);
        let kept = end_cut("retry the charge", by_chars(6));
        assert_eq!(kept, Some(5), "`retry…` is 6 characters");
        assert_eq!(end_cut("жжжж", by_chars(3)), Some("жж".len()));
        assert_eq!(end_cut("abc", by_chars(0)), Some(0));
    }

    #[test]
    fn runs_cover_the_text_with_each_highlight_once() {
        let style = TextStyle::default();
        let bold = HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() };
        let lens = |text: &str, hl: &[(Range<usize>, HighlightStyle)]| runs(text, &style, hl).iter().map(|r| r.len).collect::<Vec<_>>();
        assert_eq!(lens("Reproducer", &[(0..2, bold)]), [2, 8]);
        assert_eq!(lens("Retry the charge", &[(0..5, bold), (10..16, bold)]), [5, 5, 6]);
        assert_eq!(lens("", &[]), [0]);
        assert_eq!(lens("ab", &[(0..2, bold)]), [2]);
    }

    #[test]
    fn highlights_are_sorted_clipped_and_kept_on_boundaries() {
        let text = "héllo wörld";
        let s = HighlightStyle::default;
        // 'é' is bytes 1..3 and 'ö' 8..10: 2..4 and 7..9 cut a character. 100 is past the
        // end, 3..3 is empty, 1..6 overlaps 0..3.
        let ranges = [(7..9, s()), (7..10, s()), (0..3, s()), (1..6, s()), (2..4, s()), (100..101, s()), (3..3, s())];
        let clean: Vec<Range<usize>> = clean_highlights(text, ranges).into_iter().map(|(r, _)| r).collect();
        assert_eq!(clean, [0..3, 3..6, 7..10]);
    }
}
