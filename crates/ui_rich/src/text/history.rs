//! Undo and redo for [`Buffer`](super::Buffer): groups of edits, with consecutive typing,
//! deleting and IME composition merged into one group.

/// One replacement: `deleted` was at `start` and `inserted` took its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Byte offset where the edit happened.
    pub start: usize,
    /// The text that was removed.
    pub deleted: String,
    /// The text that was put in its place.
    pub inserted: String,
}

/// What produced an edit, which decides whether it joins the previous undo group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// A typed grapheme (not a newline) with no selection: joins contiguous typing.
    Typing,
    /// Backspace with no selection: joins the previous backspaces.
    DeleteBack,
    /// Delete-forward with no selection: joins the previous forward deletes.
    DeleteForward,
    /// IME composition: joins everything until the composition ends.
    Compose,
    /// Anything else (paste, cut, newline, word delete, replace): its own group.
    Other,
}

/// Selection as `(anchor, head)`.
pub type Sel = (usize, usize);

#[derive(Clone, Debug)]
struct Group {
    kind: EditKind,
    edits: Vec<Edit>,
    before: Sel,
    after: Sel,
}

/// The undo and redo stacks.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    /// The last group may take more edits.
    open: bool,
}

const LIMIT: usize = 1000;

impl History {
    /// Record `edit` (already applied), moving the selection from `before` to `after`.
    pub fn record(&mut self, kind: EditKind, edit: Edit, before: Sel, after: Sel) {
        self.redo.clear();
        if self.open
            && let Some(last) = self.undo.last_mut()
            && last.kind == kind
            && last
                .edits
                .last()
                .is_some_and(|prev| joins(kind, prev, &edit))
        {
            last.edits.push(edit);
            last.after = after;
            return;
        }
        self.undo.push(Group {
            kind,
            edits: vec![edit],
            before,
            after,
        });
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.open = kind != EditKind::Other;
    }

    /// The next edit starts a new group (after a caret move, a paste, a pause).
    pub fn close_group(&mut self) {
        self.open = false;
    }

    /// Pop the last group: the edits to revert, newest first, and the selection to restore.
    pub fn undo(&mut self) -> Option<(Vec<Edit>, Sel)> {
        let group = self.undo.pop()?;
        self.open = false;
        let edits = group.edits.iter().rev().map(invert).collect();
        let sel = group.before;
        self.redo.push(group);
        Some((edits, sel))
    }

    /// Pop the last undone group: the edits to apply again, oldest first, and the selection.
    pub fn redo(&mut self) -> Option<(Vec<Edit>, Sel)> {
        let group = self.redo.pop()?;
        self.open = false;
        let edits = group.edits.clone();
        let sel = group.after;
        self.undo.push(group);
        Some((edits, sel))
    }

    /// Forget everything (after the whole text is replaced).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// There is something to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// There is something to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

fn invert(e: &Edit) -> Edit {
    Edit {
        start: e.start,
        deleted: e.inserted.clone(),
        inserted: e.deleted.clone(),
    }
}

fn joins(kind: EditKind, prev: &Edit, next: &Edit) -> bool {
    match kind {
        EditKind::Typing => {
            // A space after a word starts a new group, so undo takes back a word at a time.
            let word_ends = next.inserted.starts_with(char::is_whitespace)
                && !prev.inserted.ends_with(char::is_whitespace);
            next.deleted.is_empty() && next.start == prev.start + prev.inserted.len() && !word_ends
        }
        EditKind::DeleteBack => {
            next.inserted.is_empty() && next.start + next.deleted.len() == prev.start
        }
        EditKind::DeleteForward => next.inserted.is_empty() && next.start == prev.start,
        EditKind::Compose => true,
        EditKind::Other => false,
    }
}
