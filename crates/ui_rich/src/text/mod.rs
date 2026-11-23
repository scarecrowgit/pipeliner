//! The editing core shared by [`TextArea`](crate::TextArea) and
//! [`TextInput`](crate::TextInput): the GPUI-free [`Buffer`], the element that shapes and
//! paints it, and the key bindings.
//!
//! Call [`bind_keys`] once at startup (after `ui::init`). Every binding is scoped to the
//! `TextInput` and `TextArea` key contexts, so it never fires elsewhere.

pub mod buffer;
pub(crate) mod core;
pub(crate) mod element;
pub mod history;
pub(crate) mod layout;
pub mod segment;

pub use buffer::{Buffer, LogicalLines, Motion, Token, VisualLines};

use gpui::{App, KeyBinding, actions};

actions!(
    text,
    [
        /// Caret one grapheme left.
        MoveLeft,
        /// Caret one grapheme right.
        MoveRight,
        /// Caret one visual line up (or the host's `Up`).
        MoveUp,
        /// Caret one visual line down (or the host's `Down`).
        MoveDown,
        /// Caret to the previous word start.
        WordLeft,
        /// Caret to the next word end.
        WordRight,
        /// Caret to the line start.
        LineStart,
        /// Caret to the line end.
        LineEnd,
        /// Caret to the start of the text.
        DocStart,
        /// Caret to the end of the text.
        DocEnd,
        /// Extend the selection one grapheme left.
        SelectLeft,
        /// Extend the selection one grapheme right.
        SelectRight,
        /// Extend the selection one visual line up.
        SelectUp,
        /// Extend the selection one visual line down.
        SelectDown,
        /// Extend the selection to the previous word start.
        SelectWordLeft,
        /// Extend the selection to the next word end.
        SelectWordRight,
        /// Extend the selection to the line start.
        SelectLineStart,
        /// Extend the selection to the line end.
        SelectLineEnd,
        /// Extend the selection to the start of the text.
        SelectDocStart,
        /// Extend the selection to the end of the text.
        SelectDocEnd,
        /// Select everything.
        SelectAll,
        /// Delete the selection or the grapheme before the caret.
        Backspace,
        /// Delete the selection or the grapheme after the caret.
        Delete,
        /// Delete to the previous word start.
        DeleteWordLeft,
        /// Delete to the next word end.
        DeleteWordRight,
        /// Delete to the line start.
        DeleteToLineStart,
        /// Delete to the line end.
        DeleteToLineEnd,
        /// Copy the selection.
        Copy,
        /// Cut the selection.
        Cut,
        /// Paste the clipboard.
        Paste,
        /// Take back the last edit.
        Undo,
        /// Apply the last undone edit again.
        Redo,
        /// Enter: submit or newline, by the field's submit mode.
        Enter,
        /// Shift+Enter: always a newline in a text area.
        Newline,
        /// Cmd/Ctrl+Enter: submit (unless the field never submits).
        SubmitKey,
        /// Escape.
        Cancel,
        /// Tab.
        Tab,
    ]
);

/// Bind the editing keys for the `TextInput` and `TextArea` key contexts: both `cmd-` and
/// `ctrl-` variants for clipboard, select all, undo and redo; `alt-` and `ctrl-` word
/// movement and deletion; Home/End and their shift variants.
pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for ctx in ["TextInput", "TextArea"] {
        let c = Some(ctx);
        macro_rules! bind {
            ($($key:literal => $action:expr),* $(,)?) => {
                $(bindings.push(KeyBinding::new($key, $action, c));)*
            };
        }
        bind! {
            "left" => MoveLeft, "right" => MoveRight, "up" => MoveUp, "down" => MoveDown,
            "shift-left" => SelectLeft, "shift-right" => SelectRight,
            "shift-up" => SelectUp, "shift-down" => SelectDown,
            "alt-left" => WordLeft, "ctrl-left" => WordLeft,
            "alt-right" => WordRight, "ctrl-right" => WordRight,
            "alt-shift-left" => SelectWordLeft, "ctrl-shift-left" => SelectWordLeft,
            "alt-shift-right" => SelectWordRight, "ctrl-shift-right" => SelectWordRight,
            "home" => LineStart, "end" => LineEnd, "cmd-left" => LineStart, "cmd-right" => LineEnd,
            "shift-home" => SelectLineStart, "shift-end" => SelectLineEnd,
            "cmd-shift-left" => SelectLineStart, "cmd-shift-right" => SelectLineEnd,
            "ctrl-home" => DocStart, "ctrl-end" => DocEnd, "cmd-up" => DocStart, "cmd-down" => DocEnd,
            "ctrl-shift-home" => SelectDocStart, "ctrl-shift-end" => SelectDocEnd,
            "cmd-shift-up" => SelectDocStart, "cmd-shift-down" => SelectDocEnd,
            "cmd-a" => SelectAll, "ctrl-a" => SelectAll,
            "backspace" => Backspace, "shift-backspace" => Backspace, "delete" => Delete,
            "alt-backspace" => DeleteWordLeft, "ctrl-backspace" => DeleteWordLeft,
            "alt-delete" => DeleteWordRight, "ctrl-delete" => DeleteWordRight,
            "cmd-backspace" => DeleteToLineStart, "cmd-delete" => DeleteToLineEnd,
            "cmd-c" => Copy, "ctrl-c" => Copy, "cmd-x" => Cut, "ctrl-x" => Cut,
            "cmd-v" => Paste, "ctrl-v" => Paste, "shift-insert" => Paste,
            "cmd-z" => Undo, "ctrl-z" => Undo,
            "cmd-shift-z" => Redo, "ctrl-shift-z" => Redo, "ctrl-y" => Redo,
            "enter" => Enter, "shift-enter" => Newline,
            "cmd-enter" => SubmitKey, "ctrl-enter" => SubmitKey,
            "escape" => Cancel, "tab" => Tab,
        }
    }
    cx.bind_keys(bindings);
}
