//! [`text_field`] and [`text_area_field`]: an editor inside `ui`'s [`InputFrame`], with the
//! mockups' `.input` / `.textarea` look and the focused ring, in one call.

use gpui::{
    App, ElementId, Entity, IntoElement, MouseButton, RenderOnce, SharedString, Window, div,
    prelude::*,
};
use ui::assets::IconName;
use ui::components::InputFrame;

use crate::{TextArea, TextInput};

/// A [`TextInput`] in an [`InputFrame`]: leading icon, trailing key cap, accent border and
/// glow while focused. A press anywhere on the frame focuses the input.
#[derive(IntoElement)]
pub struct TextField {
    id: ElementId,
    input: Entity<TextInput>,
    icon: Option<IconName>,
    kbd: Option<SharedString>,
    small: bool,
}

/// [`TextField`] for `input`; `id` must be unique among its siblings.
pub fn text_field(id: impl Into<ElementId>, input: &Entity<TextInput>) -> TextField {
    TextField {
        id: id.into(),
        input: input.clone(),
        icon: None,
        kbd: None,
        small: false,
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
}

impl RenderOnce for TextField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let input = self.input.read(cx);
        let focused = input.is_focused(window);
        let disabled = input.is_disabled();
        let frame = InputFrame::new(self.id)
            .focused(focused)
            .when(self.small, |f| f.small())
            .when_some(self.icon, |f, i| f.icon(i))
            .when_some(self.kbd, |f, k| f.kbd(k))
            .child(self.input.clone());
        let target = self.input.clone();
        div().w_full().child(frame).when(!disabled, |d| {
            d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                target.read(cx).focus(window)
            })
        })
    }
}

/// A [`TextArea`] in a multi-line [`InputFrame`] (the `.textarea` look): at least 72px tall,
/// text from the top, accent border and glow while focused.
#[derive(IntoElement)]
pub struct TextAreaField {
    id: ElementId,
    area: Entity<TextArea>,
}

/// [`TextAreaField`] for `area`; `id` must be unique among its siblings.
pub fn text_area_field(id: impl Into<ElementId>, area: &Entity<TextArea>) -> TextAreaField {
    TextAreaField {
        id: id.into(),
        area: area.clone(),
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
            .child(self.area.clone());
        let target = self.area.clone();
        div().w_full().child(frame).when(!disabled, |d| {
            d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                target.read(cx).focus(window)
            })
        })
    }
}
