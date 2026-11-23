//! The small label that appears after hovering an icon-only control.

use gpui::{AnyView, App, Context, IntoElement, Render, SharedString, Window, div, prelude::*, px};

use crate::theme::*;

/// A tooltip bubble: `surface-raised` with a `line-strong` border, label type.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    /// A tooltip builder for `.tooltip(Tooltip::text("Collapse sidebar"))`.
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text: SharedString = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(SPACE_2))
            .py(px(SPACE_1))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(LINE_STRONG)
            .bg(SURFACE_RAISED)
            .text_color(TEXT)
            .type_style(LABEL)
            .child(self.text.clone())
    }
}
