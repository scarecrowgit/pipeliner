use gpui::{
    div, px, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::text::TextView;

use crate::theme::{self, TextStyles as _};

/// Message text: Markdown (GFM: lists, tables, code blocks, links) with app type.
/// Needs a stable, unique id per message (`("msg", &message.id)` style).
///
/// ```ignore
/// .child(ui::markdown(SharedString::from(format!("msg-{}", m.id)), m.content.clone(), window, cx))
/// ```
pub fn markdown(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> TextView {
    TextView::markdown(id, text, window, cx)
        .selectable(true)
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(theme::TEXT_SECONDARY)
}

/// Monospace block for logs, commands, tool output. Scrolls horizontally, never wraps.
#[derive(IntoElement)]
pub struct CodeBlock {
    id: ElementId,
    text: SharedString,
    max_height: Option<gpui::Pixels>,
}

impl CodeBlock {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        CodeBlock { id: id.into(), text: text.into(), max_height: None }
    }
    /// Scroll vertically beyond this height.
    pub fn max_height(mut self, h: gpui::Pixels) -> Self {
        self.max_height = Some(h);
        self
    }
}

impl RenderOnce for CodeBlock {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut d = div()
            .id(self.id)
            .w_full()
            .p(theme::S3)
            .rounded(theme::RADIUS_LG)
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::LINE)
            .text_mono()
            .text_color(theme::TEXT_SECONDARY)
            .overflow_x_scroll();
        if let Some(h) = self.max_height {
            d = d.max_h(h).overflow_y_scroll();
        }
        d.child(div().whitespace_nowrap().flex().flex_col().children(
            self.text.lines().map(|l| div().min_h(px(20.)).child(SharedString::from(l.replace('\t', "    ")))).collect::<Vec<_>>(),
        ))
    }
}
