//! Grapheme, word and line boundaries over a UTF-8 string, and UTF-16 offset conversion.
//!
//! Every function takes byte offsets, accepts any offset (out of range or inside a
//! character) and never panics: offsets are clamped and snapped first.

use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// `offset` clamped to the text and moved down to a char boundary.
pub fn floor_char(text: &str, offset: usize) -> usize {
    let mut i = offset.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `offset` clamped to the text and moved up to a char boundary.
pub fn ceil_char(text: &str, offset: usize) -> usize {
    let mut i = offset.min(text.len());
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Whether `offset` sits between two grapheme clusters (or at either end).
pub fn is_grapheme_boundary(text: &str, offset: usize) -> bool {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return false;
    }
    GraphemeCursor::new(offset, text.len(), true)
        .is_boundary(text, 0)
        .unwrap_or(true)
}

/// The grapheme boundary before `offset` (0 at the start).
pub fn prev_grapheme(text: &str, offset: usize) -> usize {
    let offset = ceil_char(text, offset);
    if offset == 0 {
        return 0;
    }
    match GraphemeCursor::new(offset, text.len(), true).prev_boundary(text, 0) {
        Ok(Some(i)) => i,
        Ok(None) => 0,
        Err(_) => floor_char(text, offset - 1),
    }
}

/// The grapheme boundary after `offset` (the length at the end).
pub fn next_grapheme(text: &str, offset: usize) -> usize {
    let offset = floor_char(text, offset);
    if offset >= text.len() {
        return text.len();
    }
    match GraphemeCursor::new(offset, text.len(), true).next_boundary(text, 0) {
        Ok(Some(i)) => i,
        Ok(None) => text.len(),
        Err(_) => ceil_char(text, offset + 1),
    }
}

/// `offset` moved down to the nearest grapheme boundary.
pub fn floor_grapheme(text: &str, offset: usize) -> usize {
    let offset = floor_char(text, offset);
    if is_grapheme_boundary(text, offset) {
        offset
    } else {
        prev_grapheme(text, offset)
    }
}

/// `offset` moved up to the nearest grapheme boundary.
pub fn ceil_grapheme(text: &str, offset: usize) -> usize {
    let offset = ceil_char(text, offset);
    if is_grapheme_boundary(text, offset) {
        offset
    } else {
        next_grapheme(text, offset)
    }
}

/// Punctuation that word movement skips like whitespace.
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '«' | '»'
                | '—'
                | '–'
                | '…'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '„'
                | '¡'
                | '¿'
                | '·'
                | '‹'
                | '›'
        )
}

/// A UAX #29 word segment that counts as a word: it has a character that is neither
/// whitespace nor punctuation (letters, digits, `_`, emoji).
fn is_wordlike(segment: &str) -> bool {
    segment
        .chars()
        .any(|c| c == '_' || (!c.is_whitespace() && !is_punct(c)))
}

/// Where Alt+Left goes from `offset`: the start of the word at or before it, skipping any
/// whitespace and punctuation in between; 0 if there is none.
pub fn prev_word(text: &str, offset: usize) -> usize {
    let offset = floor_char(text, offset);
    text[..offset]
        .split_word_bound_indices()
        .rev()
        .find(|(_, s)| is_wordlike(s))
        .map_or(0, |(i, _)| i)
}

/// Where Alt+Right goes from `offset`: the end of the word at or after it, skipping any
/// whitespace and punctuation in between; the length if there is none.
pub fn next_word(text: &str, offset: usize) -> usize {
    let offset = ceil_char(text, offset);
    text[offset..]
        .split_word_bound_indices()
        .find(|(_, s)| is_wordlike(s))
        .map_or(text.len(), |(i, s)| offset + i + s.len())
}

/// The word segment under `offset` for a double click: the word, run of spaces or single
/// punctuation mark that starts at or contains `offset`. Past the end of a line (on its
/// `\n`, or at the end of the text) it is the segment before, if that is not a line break;
/// a line break itself is never selected (the range is empty there).
pub fn word_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char(text, offset);
    let mut last: Option<(Range<usize>, &str)> = None;
    let before = |last: Option<(Range<usize>, &str)>| match last {
        Some((r, s)) if !s.starts_with('\n') => r,
        _ => offset..offset,
    };
    for (i, s) in text.split_word_bound_indices() {
        let r = i..i + s.len();
        if r.contains(&offset) {
            if s.starts_with('\n') {
                return before(last);
            }
            return r;
        }
        last = Some((r, s));
    }
    before(last)
}

/// The logical line (between `\n`s, without them) that holds `offset`.
pub fn line_at(text: &str, offset: usize) -> Range<usize> {
    line_start(text, offset)..line_end(text, offset)
}

/// Start of the logical line holding `offset`.
pub fn line_start(text: &str, offset: usize) -> usize {
    let offset = floor_char(text, offset);
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// End of the logical line holding `offset` (before its `\n`).
pub fn line_end(text: &str, offset: usize) -> usize {
    let offset = floor_char(text, offset);
    text[offset..].find('\n').map_or(text.len(), |i| offset + i)
}

/// The UTF-16 length of `text[..offset]`.
pub fn to_utf16(text: &str, offset: usize) -> usize {
    let offset = floor_char(text, offset);
    text[..offset].chars().map(char::len_utf16).sum()
}

/// The byte offset of the UTF-16 offset `utf16`, clamped to the text. An offset inside a
/// surrogate pair resolves to the start of that character.
pub fn from_utf16(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        let next = units + c.len_utf16();
        if next > utf16 {
            return i;
        }
        units = next;
    }
    text.len()
}

/// [`to_utf16`] for both ends of a range.
pub fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    to_utf16(text, range.start)..to_utf16(text, range.end)
}

/// [`from_utf16`] for both ends of a range, ordered.
pub fn range_from_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    let (a, b) = (from_utf16(text, range.start), from_utf16(text, range.end));
    a.min(b)..a.max(b)
}

/// Line endings normalised to `\n` (`\r\n` and lone `\r`).
pub fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n").into()
    } else {
        text.into()
    }
}

/// Text for a single-line field: every line break becomes one space.
pub fn flatten_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains(['\n', '\r']) {
        text.replace("\r\n", " ").replace(['\r', '\n'], " ").into()
    } else {
        text.into()
    }
}
