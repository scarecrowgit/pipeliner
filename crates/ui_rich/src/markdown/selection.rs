//! A text selection across the units of one rendered document, without GPUI: two points
//! (unit index, byte offset), the part of each unit they cover, and the copied text.

use std::ops::Range;

use super::rich::Unit;
use crate::text::segment as seg;

/// A position in the rendered text: unit `unit`, byte `offset` of its display text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TextPoint {
    pub unit: usize,
    pub offset: usize,
}

/// Where the mouse went down (`anchor`) and where it is now (`head`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Selection {
    pub anchor: Option<TextPoint>,
    pub head: Option<TextPoint>,
}

impl Selection {
    /// Nothing selected (no points, or both at the same place).
    pub fn is_empty(&self) -> bool {
        match (self.anchor, self.head) {
            (Some(a), Some(h)) => a == h,
            _ => true,
        }
    }

    /// Start and end in reading order.
    pub fn ordered(&self) -> Option<(TextPoint, TextPoint)> {
        let (a, h) = (self.anchor?, self.head?);
        Some(if a <= h { (a, h) } else { (h, a) })
    }

    /// Put both points at `p`.
    pub fn collapse(&mut self, p: TextPoint) {
        self.anchor = Some(p);
        self.head = Some(p);
    }

    /// Select `range` of unit `unit`.
    pub fn set(&mut self, unit: usize, range: Range<usize>) {
        self.anchor = Some(TextPoint {
            unit,
            offset: range.start,
        });
        self.head = Some(TextPoint {
            unit,
            offset: range.end,
        });
    }

    /// Drop the selection.
    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
    }

    /// The byte range selected inside unit `unit` whose text is `len` bytes long; `None`
    /// when the selection does not reach it. A unit the selection passes over entirely gives
    /// its whole range, even when that is empty.
    pub fn range_in(&self, unit: usize, len: usize) -> Option<Range<usize>> {
        if self.is_empty() {
            return None;
        }
        let (s, e) = self.ordered()?;
        if unit < s.unit || unit > e.unit {
            return None;
        }
        let start = if unit == s.unit { s.offset.min(len) } else { 0 };
        let end = if unit == e.unit {
            e.offset.min(len)
        } else {
            len
        };
        (start < end || (unit > s.unit && unit < e.unit)).then_some(start..end)
    }

    /// The selected text: each unit's part, joined by the later unit's joiner.
    pub fn text(&self, units: &[Unit]) -> Option<String> {
        let mut out = String::new();
        let mut first = true;
        for (i, u) in units.iter().enumerate() {
            let t = &u.text.text;
            let Some(r) = self.range_in(i, t.len()) else {
                continue;
            };
            let (s, e) = (seg::floor_char(t, r.start), seg::floor_char(t, r.end));
            if !first {
                out.push_str(&u.joiner);
            }
            out.push_str(&t[s..e.max(s)]);
            first = false;
        }
        (!out.is_empty()).then_some(out)
    }

    /// Select everything in `units`.
    pub fn select_all(&mut self, units: &[Unit]) {
        let Some(last) = units.len().checked_sub(1) else {
            return;
        };
        self.anchor = Some(TextPoint { unit: 0, offset: 0 });
        self.head = Some(TextPoint {
            unit: last,
            offset: units[last].text.text.len(),
        });
    }
}

/// The word (or run of spaces, or single punctuation mark) at `offset`, for a double click.
pub(crate) fn word_range(text: &str, offset: usize) -> Range<usize> {
    let r = seg::word_at(text, offset);
    if r.is_empty() && offset < text.len() {
        let o = seg::floor_grapheme(text, offset);
        return o..seg::next_grapheme(text, o);
    }
    r
}
