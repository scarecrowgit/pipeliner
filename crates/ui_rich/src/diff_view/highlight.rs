//! Intra-line emphasis: the words that changed between a removed line and the added line
//! that replaced it.

use std::ops::Range;

use similar::{Algorithm, DiffTag, capture_diff_slices};

/// Byte ranges of a line to emphasise.
pub type Marks = Vec<Range<usize>>;

/// Lines longer than this (in bytes) get no emphasis: too slow and too noisy.
pub const MAX_EMPHASIS_LINE: usize = 500;

/// When more than this share of either line changed, the lines are simply different and
/// emphasis would only add noise.
const MAX_CHANGED: f32 = 0.7;

/// Changed byte ranges of `old` and of `new`, word by word; `None` when the lines are equal,
/// too long, or mostly different.
pub fn emphasis(old: &str, new: &str) -> Option<(Marks, Marks)> {
    if old == new || old.len() > MAX_EMPHASIS_LINE || new.len() > MAX_EMPHASIS_LINE {
        return None;
    }
    let (a, b) = (tokens(old), tokens(new));
    let ta: Vec<&str> = a.iter().map(|r| &old[r.clone()]).collect();
    let tb: Vec<&str> = b.iter().map(|r| &new[r.clone()]).collect();
    let mut ra = Vec::new();
    let mut rb = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &ta, &tb) {
        let (tag, oi, ni) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        if !oi.is_empty() {
            ra.push(a[oi.start].start..a[oi.end - 1].end);
        }
        if !ni.is_empty() {
            rb.push(b[ni.start].start..b[ni.end - 1].end);
        }
    }
    let (ra, rb) = (merge(old, ra), merge(new, rb));
    let changed = |t: &str, r: &[Range<usize>]| {
        let n: usize = r.iter().map(|r| r.len()).sum();
        let len = t.trim().len().max(1);
        n as f32 / len as f32
    };
    if changed(old, &ra) > MAX_CHANGED || changed(new, &rb) > MAX_CHANGED {
        return None;
    }
    if ra.is_empty() && rb.is_empty() {
        return None;
    }
    Some((ra, rb))
}

/// Words (letters, digits, `_`), runs of whitespace, and single other characters.
fn tokens(text: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum K {
        Word,
        Space,
        Other,
    }
    let kind = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            K::Word
        } else if c.is_whitespace() {
            K::Space
        } else {
            K::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut last: Option<K> = None;
    for (i, c) in text.char_indices() {
        let k = kind(c);
        let joins = k != K::Other && last.as_ref() == Some(&k);
        match out.last_mut() {
            Some(r) if joins => r.end = i + c.len_utf8(),
            _ => out.push(i..i + c.len_utf8()),
        }
        last = Some(k);
    }
    out
}

/// Sort, then join ranges that touch or are separated only by whitespace; drop ranges that
/// are only whitespace.
fn merge(text: &str, mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        if text[r.clone()].trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if r.start <= last.end || text[last.end..r.start].trim().is_empty() => {
                last.end = last.end.max(r.end);
            }
            _ => out.push(r),
        }
    }
    out
}
