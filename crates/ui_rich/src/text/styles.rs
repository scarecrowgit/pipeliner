//! Styled ranges of an editor's text (accepted `@mentions` in the composer) and the text
//! runs they turn into when the text is shaped.

use std::ops::Range;

use gpui::{FontWeight, Hsla, TextRun, UnderlineStyle, px};

use super::layout::TextLook;

/// A range of an editor's text drawn differently: a colour, a background, a weight. Byte
/// offsets into the text; the caller keeps them in step with edits. A range that does not
/// fit the text (past its end, inside a character) is not drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyledRange {
    /// Byte range in the text.
    pub range: Range<usize>,
    /// Text colour instead of the inherited one.
    pub color: Option<Hsla>,
    /// A fill behind the glyphs.
    pub background: Option<Hsla>,
    /// Font weight instead of the inherited one.
    pub weight: Option<FontWeight>,
}

impl StyledRange {
    /// `range` with nothing changed yet; add `color`, `background`, `weight`.
    pub fn new(range: Range<usize>) -> Self {
        Self { range, ..Self::default() }
    }

    /// The text colour.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// A fill behind the glyphs.
    pub fn background(mut self, color: Hsla) -> Self {
        self.background = Some(color);
        self
    }

    /// The font weight.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    /// The range fits `text`: inside it, not empty, on character boundaries.
    pub fn fits(&self, text: &str) -> bool {
        let r = &self.range;
        r.start < r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)
    }
}

/// The runs for shaping `text` with `look`: the styled ranges that fit (a later one wins
/// where two overlap) and the IME composition `marked` underlined.
pub(crate) fn runs_for(
    text: &str,
    look: &TextLook,
    marked: Option<Range<usize>>,
    styles: &[StyledRange],
) -> Vec<TextRun> {
    let len = text.len();
    let marked = marked.filter(|m| m.start < m.end && m.end <= len);
    let styles: Vec<&StyledRange> = styles.iter().filter(|s| s.fits(text)).collect();
    let mut cuts: Vec<usize> = vec![0, len];
    cuts.extend(styles.iter().flat_map(|s| [s.range.start, s.range.end]));
    if let Some(m) = &marked {
        cuts.extend([m.start, m.end]);
    }
    cuts.retain(|&c| c <= len && text.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs: Vec<TextRun> = Vec::with_capacity(cuts.len());
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let covering = styles.iter().rev().find(|s| s.range.start <= a && b <= s.range.end);
        let underline = marked.as_ref().is_some_and(|m| m.start <= a && b <= m.end);
        let mut font = look.font.clone();
        if let Some(w) = covering.and_then(|s| s.weight) {
            font.weight = w;
        }
        let color = covering.and_then(|s| s.color).unwrap_or(look.color);
        let run = TextRun {
            len: b - a,
            font,
            color,
            background_color: covering.and_then(|s| s.background),
            underline: underline.then_some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }),
            strikethrough: None,
        };
        match runs.last_mut() {
            Some(last) if same_style(last, &run) => last.len += run.len,
            _ => runs.push(run),
        }
    }
    if runs.is_empty() {
        runs.push(TextRun {
            len,
            font: look.font.clone(),
            color: look.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    runs
}

fn same_style(a: &TextRun, b: &TextRun) -> bool {
    a.font == b.font
        && a.color == b.color
        && a.background_color == b.background_color
        && a.underline == b.underline
        && a.strikethrough == b.strikethrough
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use gpui::{font, hsla};

    use super::*;

    fn look() -> TextLook {
        TextLook { font: font("Inter"), font_size: px(13.), line_height: px(20.), color: hsla(0., 0., 1., 1.) }
    }

    fn lens(runs: &[TextRun]) -> Vec<usize> {
        runs.iter().map(|r| r.len).collect()
    }

    #[test]
    fn plain_text_is_one_run() {
        assert_eq!(lens(&runs_for("hello", &look(), None, &[])), [5]);
        assert_eq!(lens(&runs_for("", &look(), None, &[])), [0]);
    }

    #[test]
    fn a_mention_gets_its_own_run_with_its_colour_and_ground() {
        let accent = hsla(0.66, 1., 0.8, 1.);
        let soft = hsla(0.66, 1., 0.5, 0.2);
        let styles = [StyledRange::new(4..12).color(accent).background(soft)];
        let runs = runs_for("Ask @Fixer3 now", &look(), None, &styles);
        assert_eq!(lens(&runs), [4, 8, 3]);
        assert_eq!(runs[1].color, accent);
        assert_eq!(runs[1].background_color, Some(soft));
        assert_eq!(runs[0].background_color, None);
    }

    #[test]
    fn ranges_that_do_not_fit_are_ignored() {
        // "é" is bytes 1..3: 2 cuts it. 40 is past the end.
        let styles = [StyledRange::new(2..4).weight(FontWeight::BOLD), StyledRange::new(3..40).weight(FontWeight::BOLD)];
        assert_eq!(lens(&runs_for("héllo", &look(), None, &styles)), [6]);
    }

    #[test]
    fn the_composition_is_underlined_inside_a_styled_range() {
        let styles = [StyledRange::new(0..6).weight(FontWeight::BOLD)];
        let runs = runs_for("abcdefgh", &look(), Some(4..8), &styles);
        assert_eq!(lens(&runs), [4, 2, 2]);
        assert!(runs[0].underline.is_none());
        assert!(runs[1].underline.is_some() && runs[1].font.weight == FontWeight::BOLD);
        assert!(runs[2].underline.is_some() && runs[2].font.weight != FontWeight::BOLD);
    }

    #[test]
    fn the_later_of_two_overlapping_ranges_wins() {
        let red = hsla(0., 1., 0.5, 1.);
        let blue = hsla(0.6, 1., 0.5, 1.);
        let styles = [StyledRange::new(0..6).color(red), StyledRange::new(3..9).color(blue)];
        let runs = runs_for("abcdefghij", &look(), None, &styles);
        assert_eq!(lens(&runs), [3, 6, 1]);
        assert_eq!((runs[0].color, runs[1].color), (red, blue));
    }
}
