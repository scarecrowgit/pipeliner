//! Minimal hover tooltip (the mockup's `title` attributes on chart bars and rows).

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, Render, SharedString, Window, div,
    prelude::*, px,
};
use ui::theme::*;

struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Tooltips render outside the sidebar's tree, so set the text style explicitly.
        div()
            .font_family(FONT_SANS)
            .text_size(px(TEXT_LABEL))
            .line_height(px(16.))
            .text_color(TEXT)
            .px(px(8.))
            .py(px(4.))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_MD))
            .shadow(popover_shadow())
            .whitespace_nowrap()
            .child(self.text.clone())
    }
}

/// Builder for `.tooltip(...)` showing `text`.
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
}
