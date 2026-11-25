//! Text inputs (gpui-component `InputState` + `Input`) with app styling.
//!
//! Create the state once (in your screen's `new`, it needs `window`) and keep the entity;
//! render it every frame with [`field`] / [`composer_field`]:
//!
//! ```ignore
//! // new():
//! let note = ui::input::composer("e.g. Keep B's approach, log at debug", window, cx);
//! let sub = cx.subscribe_in(&note, window, |this, input, ev: &InputEvent, window, cx| {
//!     if let InputEvent::PressEnter { secondary: true } = ev { this.send(window, cx) }
//! });
//! // render():
//! .child(ui::input::composer_field(&self.note))
//! // read / clear:
//! let text = self.note.read(cx).value();
//! self.note.update(cx, |s, cx| s.set_value("", window, cx));
//! ```

use gpui::{px, App, AppContext as _, Entity, SharedString, Styled as _, Window};
use gpui_component::input::{Input, InputState};

use crate::theme;

pub use gpui_component::input::InputEvent;

/// Single-line input state.
pub fn text_input(
    placeholder: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

/// Single-line input state with an initial value.
pub fn text_input_with(
    placeholder: impl Into<SharedString>,
    value: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let value = value.into();
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

/// Multi-line composer that grows from `min_rows` to `max_rows`, then scrolls.
/// Enter inserts a newline; ctrl/cmd-Enter emits `InputEvent::PressEnter { secondary: true }`.
pub fn composer(
    placeholder: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    composer_rows(placeholder, 2, 10, window, cx)
}

pub fn composer_rows(
    placeholder: impl Into<SharedString>,
    min_rows: usize,
    max_rows: usize,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    cx.new(|cx| {
        InputState::new(window, cx)
            .auto_grow(min_rows, max_rows)
            .placeholder(placeholder)
    })
}

/// Multi-line editor for long text (prompts, rules) with a fixed number of rows.
pub fn text_area(
    placeholder: impl Into<SharedString>,
    rows: usize,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    cx.new(|cx| {
        InputState::new(window, cx)
            .multi_line(true)
            .rows(rows)
            .placeholder(placeholder)
    })
}

/// Renders a single-line input: 40px, raised surface, strong border, accent focus ring.
pub fn field(state: &Entity<InputState>) -> Input {
    Input::new(state)
        .h(theme::CONTROL_H)
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_LG)
        .text_size(px(13.))
}

/// Compact 28px single-line input (filters, inline edits).
pub fn field_sm(state: &Entity<InputState>) -> Input {
    Input::new(state)
        .h(theme::CONTROL_H_SM)
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_MD)
        .text_size(px(12.))
}

/// Renders a multi-line input (composer / text area).
pub fn composer_field(state: &Entity<InputState>) -> Input {
    Input::new(state)
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_LG)
        .text_size(px(13.))
}
