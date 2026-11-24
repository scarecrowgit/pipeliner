//! [`TextArea`]: multi-line text editing for the chat composer, issue descriptions and
//! comments, the agent role editor and dialog text areas.
//!
//! ```ignore
//! let area = cx.new(|cx| TextArea::new(cx).placeholder("Message…").submit_on(SubmitOn::Enter).rows(1, 12));
//! cx.subscribe(&area, |this, area, ev: &TextAreaEvent, cx| match ev { .. }).detach();
//! // render: text_area_field("composer", &area)   or just   div().child(area.clone())
//! ```
//!
//! Font, size, line height and colour come from the parent's text style. The view grows with
//! its content between `rows(min, max)` lines and scrolls beyond, keeping the caret in view.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, EventEmitter, FocusHandle,
    Focusable, IntoElement, Pixels, Render, SharedString, Window, div, prelude::*,
};

use crate::text::core::{self, EditCore, EditorView, impl_input_handler};
use crate::text::element::TextElement;
use crate::text::{self as t, Buffer, StyledRange, Token};

/// Which key sends the text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubmitOn {
    /// Enter submits, Shift+Enter is a newline (the chat composer).
    Enter,
    /// Cmd/Ctrl+Enter submits, Enter is a newline (comments, dialogs).
    #[default]
    CmdEnter,
    /// Nothing submits; Enter is a newline and Cmd/Ctrl+Enter goes to the parent.
    Never,
}

/// What a [`TextArea`] tells its host.
#[derive(Clone, Debug, PartialEq)]
pub enum TextAreaEvent {
    /// The user changed the text (typing, IME, paste, cut, delete, undo, and the host's
    /// `insert_text` / `replace_token`). `set_text` does not emit it.
    Changed,
    /// The submit key of the [`SubmitOn`] mode.
    Submit,
    /// Escape.
    Cancel,
    /// Focus arrived.
    Focus,
    /// Focus left.
    Blur,
    /// Up, while the host intercepts arrows (an open popup); the caret did not move.
    Up,
    /// Down, while the host intercepts arrows.
    Down,
    /// Tab, while the host intercepts arrows. Otherwise Tab moves focus to the next field
    /// (or types a tab with [`TextArea::literal_tabs`]).
    Tab,
    /// Enter, while the host intercepts arrows: pick the popup's item. Nothing is inserted.
    Confirm,
    /// Paste of a clipboard item that holds images and no text; the item is attached.
    PasteImage(ClipboardItem),
}

/// A multi-line editable text field. See the module docs.
pub struct TextArea {
    core: EditCore,
    submit_on: SubmitOn,
    intercept: bool,
    literal_tabs: bool,
}

impl EventEmitter<TextAreaEvent> for TextArea {}

impl TextArea {
    /// An empty text area: 1 to 10 rows, Cmd/Ctrl+Enter submits.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            core: EditCore::new(Buffer::new(""), true, cx),
            submit_on: SubmitOn::default(),
            intercept: false,
            literal_tabs: false,
        }
    }

    /// Builder: Tab types a tab character instead of moving focus (Shift-Tab still moves
    /// focus back). Off by default: the area is a tab stop like any field.
    pub fn literal_tabs(mut self, on: bool) -> Self {
        self.literal_tabs = on;
        self
    }

    /// Builder: muted text shown while empty.
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.core.placeholder = text.into();
        self
    }

    /// Builder: which key submits.
    pub fn submit_on(mut self, mode: SubmitOn) -> Self {
        self.submit_on = mode;
        self
    }

    /// Builder: grow from `min` to `max` lines of text, then scroll.
    pub fn rows(mut self, min: usize, max: usize) -> Self {
        self.core.min_rows = min.max(1);
        self.core.max_rows = max.max(self.core.min_rows);
        self
    }

    /// Builder: initial text, caret at the end.
    pub fn with_text(mut self, text: &str) -> Self {
        self.core.buffer.set_text(text);
        self
    }

    /// Grow from `min` to `max` lines of text, then scroll (the builder [`rows`](Self::rows)
    /// after creation).
    pub fn set_rows(&mut self, min: usize, max: usize, cx: &mut Context<Self>) {
        let min = min.max(1);
        let max = max.max(min);
        if (self.core.min_rows, self.core.max_rows) != (min, max) {
            self.core.min_rows = min;
            self.core.max_rows = max;
            cx.notify();
        }
    }

    /// Draw these ranges of the text in their own style (accepted `@mentions`). The caller
    /// moves them along edits (see [`TextAreaEvent::Changed`]); ranges that no longer fit the
    /// text are skipped until they are replaced.
    pub fn set_styles(&mut self, styles: Vec<StyledRange>, cx: &mut Context<Self>) {
        if self.core.styles != styles {
            self.core.styles = styles;
            cx.notify();
        }
    }

    /// The styled ranges, as last set.
    pub fn styles(&self) -> &[StyledRange] {
        &self.core.styles
    }

    /// Whether Tab types a tab character (see [`literal_tabs`](Self::literal_tabs)).
    pub fn set_literal_tabs(&mut self, on: bool) {
        self.literal_tabs = on;
    }

    /// The text.
    pub fn text(&self) -> &str {
        self.core.buffer.text()
    }

    /// No text.
    pub fn is_empty(&self) -> bool {
        self.core.buffer.is_empty()
    }

    /// The selection as byte offsets, ordered.
    pub fn selection(&self) -> Range<usize> {
        self.core.buffer.selection()
    }

    /// The caret's byte offset.
    pub fn cursor(&self) -> usize {
        self.core.buffer.head()
    }

    /// Replace the text, caret at the end, history cleared. Does not emit `Changed`.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.core.buffer.set_text(text);
        self.core.touched();
        cx.notify();
    }

    /// Insert `text` in place of the selection, as one undo step. Emits `Changed`.
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        core::edit(self, cx, |b| b.insert(text));
    }

    /// Select everything.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.core.buffer.select_all();
        self.core.touched();
        cx.notify();
    }

    /// Select `range` (byte offsets; the caret goes to `range.end`).
    pub fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.core.buffer.select(range.start, range.end);
        self.core.touched();
        cx.notify();
    }

    /// Change the placeholder.
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.core.placeholder = text.into();
        cx.notify();
    }

    /// Change which key submits.
    pub fn set_submit_on(&mut self, mode: SubmitOn, cx: &mut Context<Self>) {
        self.submit_on = mode;
        cx.notify();
    }

    /// Read-only, muted and unfocusable while `disabled`.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.core.disabled = disabled;
        self.core.drag = None;
        cx.notify();
    }

    /// Whether the area is disabled.
    pub fn is_disabled(&self) -> bool {
        self.core.disabled
    }

    /// While on, Up, Down, Tab and Enter are emitted as events for the host's popup
    /// instead of editing.
    pub fn intercept_arrows(&mut self, on: bool) {
        self.intercept = on;
    }

    /// The `@mention` or `/command` being typed before the caret, if any.
    pub fn token_before_cursor(&self, triggers: &[char]) -> Option<Token> {
        self.core.buffer.token_before_cursor(triggers)
    }

    /// Replace `token` (trigger included) with `replacement`. Emits `Changed`.
    pub fn replace_token(
        &mut self,
        token: &Token,
        replacement: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        core::edit(self, cx, |b| b.replace_token(token, replacement))
    }

    /// The caret's rectangle in window coordinates as of the last paint, for anchoring a
    /// popup. None before the first paint.
    pub fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        self.core.caret_bounds()
    }

    /// Give the area keyboard focus (no-op while disabled).
    pub fn focus(&self, window: &mut Window) {
        if !self.core.disabled {
            self.core.focus.focus(window);
        }
    }

    /// The area has keyboard focus.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.core.focus.is_focused(window)
    }

    fn enter(&mut self, _: &t::Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.core.disabled {
            return;
        }
        if self.intercept {
            cx.emit(TextAreaEvent::Confirm);
        } else if self.submit_on == SubmitOn::Enter {
            cx.emit(TextAreaEvent::Submit);
        } else {
            self.insert_newline(cx);
        }
    }

    fn insert_newline(&mut self, cx: &mut Context<Self>) {
        if !self.core.disabled {
            core::edit(self, cx, |b| b.commit_text(None, "\n"));
        }
    }

    fn submit_key(&mut self, _: &t::SubmitKey, _: &mut Window, cx: &mut Context<Self>) {
        match self.submit_on {
            SubmitOn::Never => cx.propagate(),
            _ if self.core.disabled => {}
            _ => cx.emit(TextAreaEvent::Submit),
        }
    }

    fn up(&mut self, _: &t::MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.intercept {
            cx.emit(TextAreaEvent::Up);
        } else {
            core::vertical(self, false, false, cx);
        }
    }

    fn down(&mut self, _: &t::MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.intercept {
            cx.emit(TextAreaEvent::Down);
        } else {
            core::vertical(self, true, false, cx);
        }
    }

    fn tab(&mut self, _: &t::Tab, _: &mut Window, cx: &mut Context<Self>) {
        if self.intercept {
            cx.emit(TextAreaEvent::Tab);
        } else if self.literal_tabs && !self.core.disabled {
            core::edit(self, cx, |b| b.insert("\t"));
        } else {
            // The kit's Tab binding moves focus to the next tab stop.
            cx.propagate();
        }
    }

    fn paste(&mut self, _: &t::Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.core.disabled {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        match item.text().filter(|s| !s.is_empty()) {
            Some(text) => {
                core::edit(self, cx, |b| b.insert(&text));
            }
            None if item
                .entries()
                .iter()
                .any(|e| matches!(e, ClipboardEntry::Image(_))) =>
            {
                cx.emit(TextAreaEvent::PasteImage(item));
            }
            None => {}
        }
    }
}

impl EditorView for TextArea {
    fn core(&self) -> &EditCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut EditCore {
        &mut self.core
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextAreaEvent::Changed);
    }

    fn focus_changed(&mut self, focused: bool, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(if focused {
            TextAreaEvent::Focus
        } else {
            TextAreaEvent::Blur
        });
    }
}

impl_input_handler!(TextArea);

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.core.focus.clone()
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        core::ensure_focus_subscriptions(self, window, cx);
        let el = div()
            .flex()
            .w_full()
            .min_w_0()
            .key_context("TextArea")
            .when(!self.core.disabled, |d| {
                d.track_focus(&self.core.focus).cursor(CursorStyle::IBeam)
            });
        core::common_actions(el, cx)
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(|this, _: &t::Newline, _, cx| this.insert_newline(cx)))
            .on_action(cx.listener(Self::submit_key))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(
                cx.listener(|this, _: &t::SelectUp, _, cx| core::vertical(this, false, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &t::SelectDown, _, cx| core::vertical(this, true, true, cx)),
            )
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(|_, _: &t::Cancel, _, cx| cx.emit(TextAreaEvent::Cancel)))
            .on_action(cx.listener(Self::paste))
            .child(TextElement::new(cx.entity()))
    }
}
