//! [`text_field`] and [`text_area_field`]: an editor inside `ui`'s [`InputFrame`], with the
//! mockups' `.input` / `.textarea` look and the focused ring, in one call.

use gpui::{
    App, ElementId, Entity, IntoElement, MouseButton, RenderOnce, SharedString, Window, div,
    prelude::*,
};
use ui::assets::IconName;
use ui::components::InputFrame;
use ui::theme::{MONO, TextStyleExt};

use crate::{TextArea, TextInput};

/// A [`TextInput`] in an [`InputFrame`]: leading icon, trailing key cap, accent border and
/// glow while focused, `danger` for an invalid value, `mono` for machine text. A press
/// anywhere on the frame focuses the input, and nothing behind the frame takes focus back.
#[derive(IntoElement)]
pub struct TextField {
    id: ElementId,
    input: Entity<TextInput>,
    icon: Option<IconName>,
    kbd: Option<SharedString>,
    small: bool,
    compact: bool,
    danger: bool,
    mono: bool,
}

/// [`TextField`] for `input`; `id` must be unique among its siblings.
pub fn text_field(id: impl Into<ElementId>, input: &Entity<TextInput>) -> TextField {
    TextField {
        id: id.into(),
        input: input.clone(),
        icon: None,
        kbd: None,
        small: false,
        compact: false,
        danger: false,
        mono: false,
    }
}

impl TextField {
    /// A 14px muted icon before the text.
    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    /// A key cap after the text ("⌘K").
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    /// The 32px compact frame.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// The 28px frame with small (12px) text and icon, for a filter field in a toolbar row
    /// ([`InputFrame::compact`]).
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// The `danger` border and halo: the value is invalid.
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    /// The text in mono (ids, paths, expressions).
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

/// `editor` as it is, or in a mono wrapper.
fn mono_wrap(mono: bool, editor: gpui::AnyElement) -> gpui::AnyElement {
    if mono {
        div().w_full().min_w_0().type_style(MONO).child(editor).into_any_element()
    } else {
        editor
    }
}

/// Focus `focus` on a press, and keep the press from reaching the window's fallback focus
/// (the overlay host) behind the frame, which would take focus straight back.
fn focus_on_press(el: gpui::Div, focus: impl Fn(&mut Window, &mut App) + 'static) -> gpui::Div {
    el.on_mouse_down(MouseButton::Left, move |_, window, cx| {
        focus(window, cx);
        window.prevent_default();
    })
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let input = self.input.read(cx);
        let focused = input.is_focused(window);
        let disabled = input.is_disabled();
        let frame = InputFrame::new(self.id)
            .focused(focused)
            .danger(self.danger)
            .when(self.small, |f| f.small())
            .when(self.compact, |f| f.compact())
            .when_some(self.icon, |f, i| f.icon(i))
            .when_some(self.kbd, |f, k| f.kbd(k))
            .child(mono_wrap(self.mono, self.input.clone().into_any_element()));
        let target = self.input.clone();
        div().w_full().child(frame).when(!disabled, |d| {
            focus_on_press(d, move |window, cx| target.read(cx).focus(window))
        })
    }
}

/// A [`TextArea`] in a multi-line [`InputFrame`] (the `.textarea` look): at least 72px tall,
/// text from the top, accent border and glow while focused, `danger` for an invalid value,
/// `mono` for machine text. A press anywhere on the frame, the empty space under the text
/// included, focuses the area.
#[derive(IntoElement)]
pub struct TextAreaField {
    id: ElementId,
    area: Entity<TextArea>,
    danger: bool,
    mono: bool,
}

/// [`TextAreaField`] for `area`; `id` must be unique among its siblings.
pub fn text_area_field(id: impl Into<ElementId>, area: &Entity<TextArea>) -> TextAreaField {
    TextAreaField {
        id: id.into(),
        area: area.clone(),
        danger: false,
        mono: false,
    }
}

impl TextAreaField {
    /// The `danger` border and halo: the value is invalid.
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    /// The text in mono (prompts with code, expressions).
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

impl RenderOnce for TextAreaField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let area = self.area.read(cx);
        let focused = area.is_focused(window);
        let disabled = area.is_disabled();
        let frame = InputFrame::new(self.id)
            .multiline()
            .focused(focused)
            .danger(self.danger)
            .child(mono_wrap(self.mono, self.area.clone().into_any_element()));
        let target = self.area.clone();
        div().w_full().child(frame).when(!disabled, |d| {
            focus_on_press(d, move |window, cx| target.read(cx).focus(window))
        })
    }
}
