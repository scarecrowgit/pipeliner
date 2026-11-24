//! Greedy line breaking over measured text, without GPUI.
//!
//! GPUI wraps a text with the metrics of its first font, so prose with mono inline code ends
//! up wider than its column. The layout here measures every run in its own font and size and
//! breaks lines itself: after spaces where possible, between graphemes inside a word that is
//! wider than the line.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// Break `text` (one logical line, no `\n`) into visual lines no wider than `width`.
///
/// `left(o)` is the x where a line starting at byte `o` begins; `right(o)` the x where a line
/// ending at `o` ends (they differ around padded spans). Spaces at a break stay at the end of
/// the earlier line and do not count towards its width; spaces that start a line (indented
/// verbatim text) are not a break, so an indent stays with its word. Every line holds at
/// least one grapheme, so a too-narrow width still terminates.
pub(crate) fn wrap(
    text: &str,
    width: f32,
    left: impl Fn(usize) -> f32,
    right: impl Fn(usize) -> f32,
) -> Vec<Range<usize>> {
    let gs: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    let end_of = |k: usize| gs.get(k + 1).copied().unwrap_or(text.len());
    let fits = |s: usize, e: usize| right(e) - left(s) <= width + 0.01;
    let mut out = Vec::new();
    let mut start = 0;
    let mut brk: Option<usize> = None;
    let mut prev_space = false;
    // A non-space grapheme came before: a run of spaces after it is a break. Every line
    // after the first starts with one, so only the first line's indent is not a break.
    let mut worded = false;
    for (k, &i) in gs.iter().enumerate() {
        let space = text[i..].starts_with(' ');
        if !space && prev_space && i > start && worded {
            brk = Some(i);
        }
        prev_space = space;
        worded |= !space;
        if space || i == start {
            continue;
        }
        let end = end_of(k);
        if fits(start, end) {
            continue;
        }
        let at = brk.filter(|&b| b > start).unwrap_or(i);
        out.push(start..at);
        start = at;
        brk = None;
        // The word now starting the line can itself be too wide: cut it between graphemes.
        while start < i && !fits(start, end) {
            let first = gs.partition_point(|&g| g <= start);
            let mut cut = gs.get(first).copied().unwrap_or(i).min(i);
            for &g in &gs[first..] {
                if g > i || !fits(start, g) {
                    break;
                }
                cut = g;
            }
            out.push(start..cut);
            start = cut;
        }
    }
    out.push(start..text.len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, width: f32) -> Vec<&str> {
        // 1 px per byte, like a mono font.
        wrap(text, width, |o| o as f32, |o| o as f32)
            .into_iter()
            .map(|r| &text[r])
            .collect()
    }

    #[test]
    fn breaks_after_spaces() {
        assert_eq!(lines("aaa bbb ccc", 7.), ["aaa bbb ", "ccc"]);
        assert_eq!(lines("aaa bbb ccc", 100.), ["aaa bbb ccc"]);
        assert_eq!(lines("aaa  bbb", 4.), ["aaa  ", "bbb"]);
    }

    #[test]
    fn cuts_long_words() {
        assert_eq!(lines("abcdefghij", 4.), ["abcd", "efgh", "ij"]);
        assert_eq!(lines("ab cdefghij", 4.), ["ab ", "cdef", "ghij"]);
        assert_eq!(lines("abc", 0.), ["a", "b", "c"]);
    }

    #[test]
    fn an_indent_stays_with_its_word() {
        assert_eq!(lines("    indented words", 12.), ["    indented ", "words"]);
        assert_eq!(lines("  ab cd", 4.), ["  ab ", "cd"]);
        assert_eq!(lines("    indented", 6.), ["    in", "dented"]);
    }

    #[test]
    fn keeps_graphemes_whole() {
        let t = "e\u{301}e\u{301}e\u{301}";
        for l in lines(t, 3.) {
            assert!(!l.is_empty() && l.starts_with('e'));
        }
        assert_eq!(lines("", 10.), [""]);
    }

    #[test]
    fn padded_spans_count() {
        // A 4px pad on each side of bytes 4..8 ("bbbb").
        let left = |o: usize| o as f32 + if o > 4 { 4. } else { 0. } + if o >= 8 { 4. } else { 0. };
        let right =
            |o: usize| o as f32 + if o >= 4 { 4. } else { 0. } + if o >= 8 { 4. } else { 0. };
        let text = "aaa bbbb c";
        let got: Vec<&str> = wrap(text, 14., left, right)
            .into_iter()
            .map(|r| &text[r])
            .collect();
        assert_eq!(got, ["aaa ", "bbbb c"]);
        let got: Vec<&str> = wrap(text, 13., left, right)
            .into_iter()
            .map(|r| &text[r])
            .collect();
        assert_eq!(got, ["aaa ", "bbbb ", "c"]);
    }
}
