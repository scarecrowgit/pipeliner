//! [`TextInput`]: the single-line face of the editor, for search fields, titles and dialog
//! inputs.
//!
//! ```ignore
//! let input = cx.new(|cx| TextInput::new(cx).placeholder("Search issues"));
//! cx.subscribe(&input, |this, input, ev: &TextInputEvent, cx| match ev { .. }).detach();
//! // render: text_field("search", &input).icon(IconName::Search).kbd("⌘K")
//! ```
//!
//! Line breaks in pasted text become spaces. Enter submits; Up, Down and Tab are emitted for
//! list navigation and completion. Long text scrolls horizontally to keep the caret visible.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, EventEmitter, FocusHandle, Focusable,
    IntoElement, Pixels, Render, SharedString, Window, div, prelude::*,
};

use crate::text::core::{self, EditCore, EditorView, impl_input_handler};
use crate::text::element::TextElement;
use crate::text::{self as t, Buffer, Token};

/// What a [`TextInput`] tells its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The user changed the text (typing, IME, paste, cut, delete, undo, and the host's
    /// `insert_text` / `replace_token`). `set_text` does not emit it.
    Changed,
    /// Enter.
    Submit,
    /// Escape.
    Cancel,
    /// Focus arrived.
    Focus,
    /// Focus left.
    Blur,
    /// Up: move a list selection.
    Up,
    /// Down: move a list selection.
    Down,
    /// Tab: complete. Not emitted with [`TextInput::tab_moves_focus`]: Tab then moves focus.
    Tab,
}

/// A single-line editable text field. See the module docs.
pub struct TextInput {
    core: EditCore,
    tab_moves_focus: bool,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    /// An empty input.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            core: EditCore::new(Buffer::single_line(""), false, cx),
            tab_moves_focus: false,
        }
    }

    /// Builder: Tab moves focus to the next field (as Shift-Tab always moves it back)
    /// instead of being emitted as [`TextInputEvent::Tab`] for completion. For form fields.
    pub fn tab_moves_focus(mut self, on: bool) -> Self {
        self.tab_moves_focus = on;
        self
    }

    /// Whether Tab moves focus (see the builder [`tab_moves_focus`](Self::tab_moves_focus)).
    pub fn set_tab_moves_focus(&mut self, on: bool) {
        self.tab_moves_focus = on;
    }

    /// Builder: muted text shown while empty.
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.core.placeholder = text.into();
        self
    }

    /// Builder: initial text, caret at the end.
    pub fn with_text(mut self, text: &str) -> Self {
        self.core.buffer.set_text(text);
        self
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

    /// Replace the text (line breaks become spaces), caret at the end, history and
    /// protected suffix cleared. Does not emit `Changed`.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.core.buffer.set_text(text);
        self.core.touched();
        cx.notify();
    }

    /// Insert `text` in place of the selection, as one undo step. Emits `Changed`.
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        core::edit(self, cx, |b| b.insert(text));
    }

    /// Make the last `bytes` of the text read-only and unreachable (`.md` during rename).
    pub fn set_protected_suffix(&mut self, bytes: usize, cx: &mut Context<Self>) {
        self.core.buffer.set_protected_suffix(bytes);
        cx.notify();
    }

    /// Select everything editable.
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

    /// Read-only, muted and unfocusable while `disabled`.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.core.disabled = disabled;
        self.core.drag = None;
        cx.notify();
    }

    /// Whether the input is disabled.
    pub fn is_disabled(&self) -> bool {
        self.core.disabled
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

    /// The caret's rectangle in window coordinates as of the last paint.
    pub fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        self.core.caret_bounds()
    }

    /// Give the input keyboard focus (no-op while disabled).
    pub fn focus(&self, window: &mut Window) {
        if !self.core.disabled {
            self.core.focus.focus(window);
        }
    }

    /// The input has keyboard focus.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.core.focus.is_focused(window)
    }

    fn paste(&mut self, _: &t::Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.core.disabled {
            return;
        }
        if let Some(text) = cx
            .read_from_clipboard()
            .and_then(|item: ClipboardItem| item.text())
        {
            core::edit(self, cx, |b| b.insert(&text));
        }
    }
}

impl EditorView for TextInput {
    fn core(&self) -> &EditCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut EditCore {
        &mut self.core
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Changed);
    }

    fn focus_changed(&mut self, focused: bool, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(if focused {
            TextInputEvent::Focus
        } else {
            TextInputEvent::Blur
        });
    }
}

impl_input_handler!(TextInput);

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.core.focus.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        core::ensure_focus_subscriptions(self, window, cx);
        let el = div()
            .flex()
            .flex_1()
            .w_full()
            .min_w_0()
            .items_center()
            .overflow_hidden()
            .key_context("TextInput")
            .when(!self.core.disabled, |d| {
                d.track_focus(&self.core.focus).cursor(CursorStyle::IBeam)
            });
        core::common_actions(el, cx)
            .on_action(cx.listener(|_, _: &t::Enter, _, cx| cx.emit(TextInputEvent::Submit)))
            .on_action(cx.listener(|_, _: &t::SubmitKey, _, cx| cx.emit(TextInputEvent::Submit)))
            // Shift+Enter would type a line break; a single line has none.
            .on_action(cx.listener(|_, _: &t::Newline, _, _| {}))
            .on_action(cx.listener(|_, _: &t::MoveUp, _, cx| cx.emit(TextInputEvent::Up)))
            .on_action(cx.listener(|_, _: &t::MoveDown, _, cx| cx.emit(TextInputEvent::Down)))
            .on_action(cx.listener(|this, _: &t::SelectUp, _, cx| {
                core::motion(this, t::Motion::DocStart, true, cx)
            }))
            .on_action(cx.listener(|this, _: &t::SelectDown, _, cx| {
                core::motion(this, t::Motion::DocEnd, true, cx)
            }))
            .on_action(cx.listener(|this, _: &t::Tab, _, cx| {
                if this.tab_moves_focus {
                    // The kit's Tab binding moves focus to the next tab stop.
                    cx.propagate();
                } else {
                    cx.emit(TextInputEvent::Tab);
                }
            }))
            .on_action(cx.listener(|_, _: &t::Cancel, _, cx| cx.emit(TextInputEvent::Cancel)))
            .on_action(cx.listener(Self::paste))
            .child(TextElement::new(cx.entity()))
    }
}
