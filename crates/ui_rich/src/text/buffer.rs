//! The editing model behind every text field: text, selection, IME composition and undo,
//! with no GPUI in it.
//!
//! Offsets are UTF-8 byte offsets. The selection's anchor and head always sit on grapheme
//! boundaries, so the caret never lands inside an emoji or between a letter and its accent.
//! Line breaks are stored as `\n` only (pasted `\r\n` and `\r` are normalised), and a
//! single-line buffer turns every line break into a space.

use std::ops::Range;

use super::history::{Edit, EditKind, History};
use super::segment::{self as seg, floor_grapheme};

/// A caret movement that does not depend on layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// One grapheme back.
    Left,
    /// One grapheme forward.
    Right,
    /// To the start of the previous word.
    WordLeft,
    /// To the end of the next word.
    WordRight,
    /// To the start of the logical line.
    LineStart,
    /// To the end of the logical line.
    LineEnd,
    /// To the start of the text.
    DocStart,
    /// To the end of the (editable) text.
    DocEnd,
}

/// The geometry vertical movement needs: the text's visual (wrapped) lines. The element
/// implements it over shaped text; tests implement it with a fixed-width fake.
pub trait VisualLines {
    /// Number of visual lines (at least 1).
    fn line_count(&self) -> usize;
    /// The visual line showing the caret at `offset`. At a soft wrap the caret belongs to
    /// the line that starts there.
    fn line_for_offset(&self, offset: usize) -> usize;
    /// Horizontal position of the caret at `offset` on its visual line.
    fn x_for_offset(&self, offset: usize) -> f32;
    /// The caret offset on visual line `line` closest to `x`.
    fn offset_for_x(&self, line: usize, x: f32) -> usize;
}

/// Visual lines when nothing is laid out yet: one per logical line, one unit per grapheme.
pub struct LogicalLines<'a>(pub &'a str);

impl VisualLines for LogicalLines<'_> {
    fn line_count(&self) -> usize {
        self.0.split('\n').count()
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        self.0[..seg::floor_char(self.0, offset)]
            .matches('\n')
            .count()
    }

    fn x_for_offset(&self, offset: usize) -> f32 {
        let start = seg::line_start(self.0, offset);
        let offset = seg::floor_char(self.0, offset);
        unicode_segmentation::UnicodeSegmentation::graphemes(&self.0[start..offset], true).count()
            as f32
    }

    fn offset_for_x(&self, line: usize, x: f32) -> usize {
        let start = self
            .0
            .split('\n')
            .take(line)
            .map(|l| l.len() + 1)
            .sum::<usize>();
        let start = start.min(self.0.len());
        let end = seg::line_end(self.0, start);
        let n = x.round().max(0.) as usize;
        unicode_segmentation::UnicodeSegmentation::grapheme_indices(&self.0[start..end], true)
            .nth(n)
            .map_or(end, |(i, _)| start + i)
    }
}

/// A mention or command being typed: the trigger character at a word start and the text
/// after it up to the caret (`@re` is `Token { trigger: '@', start, text: "re" }`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The trigger character, e.g. `@` or `/`.
    pub trigger: char,
    /// Byte offset of the trigger.
    pub start: usize,
    /// What follows the trigger, up to the caret.
    pub text: String,
}

/// Text plus selection, composition and history. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    text: String,
    anchor: usize,
    head: usize,
    marked: Option<Range<usize>>,
    goal_x: Option<f32>,
    history: History,
    single_line: bool,
    protected: usize,
}

impl Buffer {
    /// A multi-line buffer holding `text`, caret at the end.
    pub fn new(text: &str) -> Self {
        let mut b = Self::default();
        b.set_text(text);
        b
    }

    /// A single-line buffer holding `text` (line breaks become spaces), caret at the end.
    pub fn single_line(text: &str) -> Self {
        let mut b = Self {
            single_line: true,
            ..Self::default()
        };
        b.set_text(text);
        b
    }

    /// Whether line breaks are turned into spaces.
    pub fn is_single_line(&self) -> bool {
        self.single_line
    }

    /// The whole text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// No text at all.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replace everything, put the caret at the end and forget the history.
    pub fn set_text(&mut self, text: &str) {
        self.text = self.clean(text);
        self.protected = 0;
        self.marked = None;
        self.goal_x = None;
        self.history.clear();
        let end = self.text.len();
        self.set_sel(end, end);
    }

    /// Make the last `bytes` of the text read-only and unreachable by the caret (a file
    /// extension during rename). Snapped to a grapheme boundary.
    pub fn set_protected_suffix(&mut self, bytes: usize) {
        let end = floor_grapheme(&self.text, self.text.len().saturating_sub(bytes));
        self.protected = self.text.len() - end;
        self.set_sel(self.anchor, self.head);
    }

    /// Where the editable part of the text ends.
    pub fn editable_end(&self) -> usize {
        self.text.len() - self.protected
    }

    /// The selection, ordered.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// The fixed end of the selection.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The moving end of the selection: where the caret is drawn.
    pub fn head(&self) -> usize {
        self.head
    }

    /// The head is before the anchor.
    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// Something is selected.
    pub fn has_selection(&self) -> bool {
        self.anchor != self.head
    }

    /// The selected text ("" when nothing is selected).
    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    /// The IME composition range, if a composition is in progress.
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    fn clean(&self, text: &str) -> String {
        if self.single_line {
            seg::flatten_newlines(text).into_owned()
        } else {
            seg::normalize_newlines(text).into_owned()
        }
    }

    fn snap(&self, offset: usize) -> usize {
        floor_grapheme(&self.text, offset.min(self.editable_end()))
    }

    fn set_sel(&mut self, anchor: usize, head: usize) {
        self.anchor = self.snap(anchor);
        self.head = self.snap(head);
    }

    fn after_move(&mut self) {
        self.goal_x = None;
        self.marked = None;
        self.history.close_group();
    }

    /// Select `anchor..head` (head is where the caret goes); both snap to graphemes.
    pub fn select(&mut self, anchor: usize, head: usize) {
        self.set_sel(anchor, head);
        self.after_move();
    }

    /// Put the caret at `offset` with nothing selected.
    pub fn set_cursor(&mut self, offset: usize) {
        self.select(offset, offset);
    }

    /// Keep the anchor and move the head to `offset` (shift-click, drag).
    pub fn extend_to(&mut self, offset: usize) {
        self.select(self.anchor, offset);
    }

    /// Select the whole editable text.
    pub fn select_all(&mut self) {
        self.select(0, self.editable_end());
    }

    /// Select the word (or run of spaces, or punctuation mark) at `offset`.
    pub fn select_word_at(&mut self, offset: usize) {
        let r = seg::word_at(&self.text, offset);
        self.select(r.start, r.end);
    }

    /// Select the logical line at `offset`, without its line break.
    pub fn select_line_at(&mut self, offset: usize) {
        let r = seg::line_at(&self.text, offset);
        self.select(r.start, r.end);
    }

    /// Select from the start or end of `origin` (what a double or triple click picked) to
    /// the far end of `piece` (the word or line under the mouse now), for dragging by words
    /// or lines.
    pub fn select_spanning(&mut self, origin: Range<usize>, piece: Range<usize>) {
        if piece.start < origin.start {
            self.select(origin.end, piece.start);
        } else {
            self.select(origin.start, piece.end.max(origin.end));
        }
    }

    fn target(&self, motion: Motion, from: usize) -> usize {
        let t = &self.text;
        let to = match motion {
            Motion::Left => seg::prev_grapheme(t, from),
            Motion::Right => seg::next_grapheme(t, from),
            Motion::WordLeft => seg::prev_word(t, from),
            Motion::WordRight => seg::next_word(t, from),
            Motion::LineStart => seg::line_start(t, from),
            Motion::LineEnd => seg::line_end(t, from),
            Motion::DocStart => 0,
            Motion::DocEnd => self.editable_end(),
        };
        self.snap(to)
    }

    /// Move the caret, or the head of the selection when `extend`. Left and Right without
    /// `extend` first collapse a selection to its start or end.
    pub fn move_to(&mut self, motion: Motion, extend: bool) {
        let sel = self.selection();
        if !extend && self.has_selection() && matches!(motion, Motion::Left | Motion::Right) {
            let o = if motion == Motion::Left {
                sel.start
            } else {
                sel.end
            };
            self.select(o, o);
            return;
        }
        let to = self.target(motion, self.head);
        if extend {
            self.select(self.anchor, to)
        } else {
            self.select(to, to)
        }
    }

    /// Move up or down one visual line, keeping the goal column across repeated moves.
    /// Up on the first line goes to the start, down on the last line to the end.
    pub fn move_vertically(&mut self, down: bool, extend: bool, lines: &dyn VisualLines) {
        let sel = self.selection();
        let from = match (extend || !self.has_selection(), down) {
            (true, _) => self.head,
            (false, true) => sel.end,
            (false, false) => sel.start,
        };
        let goal = self.goal_x.unwrap_or_else(|| lines.x_for_offset(from));
        let line = lines.line_for_offset(from);
        let last = lines.line_count().saturating_sub(1);
        let to = match (down, line) {
            (false, 0) => 0,
            (true, l) if l >= last => self.editable_end(),
            (true, l) => lines.offset_for_x(l + 1, goal),
            (false, l) => lines.offset_for_x(l - 1, goal),
        };
        if extend {
            self.select(self.anchor, to)
        } else {
            self.select(to, to)
        }
        self.goal_x = Some(goal);
    }

    /// The goal column kept across vertical moves, if the last move was vertical.
    pub fn goal_x(&self) -> Option<f32> {
        self.goal_x
    }

    /// Replace `range` with `text` (cleaned) and put the caret after it. Returns the range
    /// of the new text, or None when nothing changed. Ranges are clamped to the editable part
    /// and snapped to characters.
    fn edit_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        kind: EditKind,
    ) -> Option<Range<usize>> {
        let text = self.clean(text);
        let limit = self.editable_end();
        let start = seg::floor_char(&self.text, range.start.min(range.end)).min(limit);
        let end = seg::ceil_char(&self.text, range.start.max(range.end))
            .min(limit)
            .max(start);
        if start == end && text.is_empty() {
            return None;
        }
        let before = (self.anchor, self.head);
        let deleted = self.text[start..end].to_string();
        self.text.replace_range(start..end, &text);
        let new = start..start + text.len();
        let caret = seg::ceil_grapheme(&self.text, new.end).min(self.editable_end());
        self.anchor = caret;
        self.head = caret;
        self.goal_x = None;
        if deleted != text {
            self.history.record(
                kind,
                Edit {
                    start,
                    deleted,
                    inserted: text,
                },
                before,
                (caret, caret),
            );
        }
        Some(new)
    }

    fn edit(&mut self, range: Range<usize>, text: &str, kind: EditKind) -> bool {
        self.edit_range(range, text, kind).is_some()
    }

    /// Replace the selection with `text` as one undo step (paste, insert from a menu).
    pub fn insert(&mut self, text: &str) -> bool {
        self.marked = None;
        self.history.close_group();
        let changed = self.edit(self.selection(), text, EditKind::Other);
        self.history.close_group();
        changed
    }

    /// Replace `range` with `text` as one undo step, caret after it.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) -> bool {
        self.marked = None;
        self.history.close_group();
        let changed = self.edit(range, text, EditKind::Other);
        self.history.close_group();
        changed
    }

    /// Text arriving from the keyboard or the IME: replaces `range`, else the composition,
    /// else the selection, and ends any composition. Consecutive typed graphemes are one
    /// undo step.
    pub fn commit_text(&mut self, range: Option<Range<usize>>, text: &str) -> bool {
        let composing = self.marked.is_some();
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let one_grapheme =
            unicode_segmentation::UnicodeSegmentation::graphemes(text, true).count() == 1;
        let kind = if composing {
            EditKind::Compose
        } else if one_grapheme && !text.contains(['\n', '\r']) {
            if !range.is_empty() {
                self.history.close_group();
            }
            EditKind::Typing
        } else {
            self.history.close_group();
            EditKind::Other
        };
        self.marked = None;
        let changed = self.edit(range, text, kind);
        if composing || kind == EditKind::Other {
            self.history.close_group();
        }
        changed
    }

    /// IME composition text: replaces `range`, else the composition, else the selection,
    /// and marks the new text. `selected` is the selection inside the new text (bytes,
    /// relative to its start); without it the caret goes after the text.
    pub fn mark_text(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) -> bool {
        if self.marked.is_none() {
            self.history.close_group();
        }
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let Some(new) = self.edit_range(range, text, EditKind::Compose) else {
            self.marked = None;
            return false;
        };
        self.marked = (!new.is_empty()).then_some(new.clone());
        if let Some(sel) = selected {
            let a = (new.start + sel.start).min(new.end);
            let b = (new.start + sel.end).min(new.end);
            self.anchor = self.snap(a);
            self.head = self.snap(b);
        }
        true
    }

    /// End the composition, keeping its text.
    pub fn unmark(&mut self) {
        if self.marked.take().is_some() {
            self.history.close_group();
        }
    }

    /// Delete the selection, or from the caret to where `motion` would move it.
    pub fn delete(&mut self, motion: Motion) -> bool {
        self.marked = None;
        if self.has_selection() {
            self.history.close_group();
            let changed = self.edit(self.selection(), "", EditKind::Other);
            self.history.close_group();
            return changed;
        }
        let to = self.target(motion, self.head);
        let range = to.min(self.head)..to.max(self.head);
        let kind = match motion {
            Motion::Left => EditKind::DeleteBack,
            Motion::Right => EditKind::DeleteForward,
            _ => EditKind::Other,
        };
        if kind == EditKind::Other {
            self.history.close_group();
        }
        let changed = self.edit(range, "", kind);
        if kind == EditKind::Other {
            self.history.close_group();
        }
        changed
    }

    /// Take back the last group of edits. Returns whether anything changed.
    pub fn undo(&mut self) -> bool {
        self.marked = None;
        let Some((edits, sel)) = self.history.undo() else {
            return false;
        };
        self.apply_all(edits, sel);
        true
    }

    /// Apply the last undone group again. Returns whether anything changed.
    pub fn redo(&mut self) -> bool {
        self.marked = None;
        let Some((edits, sel)) = self.history.redo() else {
            return false;
        };
        self.apply_all(edits, sel);
        true
    }

    fn apply_all(&mut self, edits: Vec<Edit>, (anchor, head): (usize, usize)) {
        for e in edits {
            let range = e.start..e.start + e.deleted.len();
            // History ranges are valid by construction; skip rather than panic if not.
            if self.text.get(range.clone()) == Some(e.deleted.as_str()) {
                self.text.replace_range(range, &e.inserted);
            }
        }
        self.goal_x = None;
        self.set_sel(anchor, head);
    }

    /// Something can be undone.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Something can be redone.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The next edit starts a new undo group.
    pub fn break_undo_group(&mut self) {
        self.history.close_group();
    }

    /// UTF-16 offset of the byte offset `offset` (what the platform input handler speaks).
    pub fn to_utf16(&self, offset: usize) -> usize {
        seg::to_utf16(&self.text, offset)
    }

    /// Byte offset of the UTF-16 offset `utf16`, clamped to the text.
    pub fn from_utf16(&self, utf16: usize) -> usize {
        seg::from_utf16(&self.text, utf16)
    }

    /// [`Self::to_utf16`] for a range.
    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        seg::range_to_utf16(&self.text, range)
    }

    /// [`Self::from_utf16`] for a range.
    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        seg::range_from_utf16(&self.text, range)
    }

    /// The mention or command being typed at the caret: the whitespace-delimited word
    /// before the caret, if it starts with one of `triggers`. None with a selection.
    pub fn token_before_cursor(&self, triggers: &[char]) -> Option<Token> {
        if self.has_selection() {
            return None;
        }
        let before = &self.text[..self.head];
        let start = before
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        let word = &before[start..];
        let trigger = word.chars().next().filter(|c| triggers.contains(c))?;
        Some(Token {
            trigger,
            start,
            text: word[trigger.len_utf8()..].to_string(),
        })
    }

    /// Replace a token found by [`Self::token_before_cursor`] (trigger included) with
    /// `replacement`, caret after it. If the text moved on since, the token at the caret
    /// with the same trigger is replaced instead; returns false when there is none.
    pub fn replace_token(&mut self, token: &Token, replacement: &str) -> bool {
        let typed = format!("{}{}", token.trigger, token.text);
        let start = if self.text.get(token.start..self.head) == Some(typed.as_str()) {
            token.start
        } else {
            match self.token_before_cursor(&[token.trigger]) {
                Some(t) => t.start,
                None => return false,
            }
        };
        self.replace_range(start..self.head, replacement)
    }
}

#[cfg(test)]
mod tests;
