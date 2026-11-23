//! The one tooltip of the app: the label that appears after hovering a control that has no
//! text of its own (icon buttons, rail buttons), with an optional key cap.
//!
//! Use it through GPUI's `.tooltip(..)` on any element with an id:
//! `.tooltip(Tooltip::text("Collapse sidebar"))` or
//! `.tooltip(Tooltip::with_kbd("New chat", "⌘N"))`. [`IconButton::tooltip`](crate::components::IconButton::tooltip)
//! does this for you.

use gpui::{AnyView, App, Context, IntoElement, Render, SharedString, Window, div, prelude::*, px};

use crate::components::Kbd;
use crate::theme::*;

/// A tooltip bubble: `surface-raised` with a `line-strong` border, label type, one line.
pub struct Tooltip {
    text: SharedString,
    kbd: Option<SharedString>,
}

impl Tooltip {
    /// A tooltip builder for `.tooltip(Tooltip::text("Collapse sidebar"))`.
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        Self::build(text.into(), None)
    }

    /// A tooltip with a key cap after the text: `Tooltip::with_kbd("New chat", "⌘N")`.
    pub fn with_kbd(
        text: impl Into<SharedString>,
        keys: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        Self::build(text.into(), Some(keys.into()))
    }

    fn build(
        text: SharedString,
        kbd: Option<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |_, cx| cx.new(|_| Tooltip { text: text.clone(), kbd: kbd.clone() }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Tooltips are drawn outside the window's tree: set the whole text style here.
        let mut el = div()
            .flex()
            .items_center()
            .gap(px(SPACE_1_5))
            .px(px(SPACE_2))
            .py(px(SPACE_1))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(LINE_STRONG)
            .bg(SURFACE_RAISED)
            .text_color(TEXT)
            .type_style(LABEL)
            .whitespace_nowrap();
        let style = gpui::Styled::text_style(&mut el).get_or_insert_with(Default::default);
        style.font_features = Some(sans_features(false));
        el.child(self.text.clone()).when_some(self.kbd.clone(), |d, k| d.child(Kbd::new(k)))
    }
}
