//! Page header: title, tagline and the primary "New" button with its create menu.

use gpui::{Context, FontWeight, IntoElement, deferred, div, prelude::*, px};
use ui::Button;
use ui::nav::{Screen, navigate};
use ui::theme::*;

use crate::AgentsScreen;
use crate::data::{Kind, NEW_KINDS};
use crate::looks::{kind_looks, tile};

impl AgentsScreen {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.new_open;
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .min_h(px(HEADER_HEIGHT))
            .px(px(SPACE_6))
            .py(px(SPACE_2))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .text_size(px(TEXT_HEADING))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Agents & pipelines"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child("One list. Anything here can take an issue, answer in chat or sit inside a pipeline."),
            )
            .child(div().flex_1())
            .child(
                div()
                    .relative()
                    .child(
                        Button::new("agents-new", "New")
                            .primary()
                            .icon("plus")
                            .on_click(cx.listener(move |this, _, _, cx| this.set_new_open(!open, cx))),
                    )
                    .when(open, |d| d.child(deferred(self.render_new_menu(cx)).with_priority(1))),
            )
    }

    fn render_new_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("agents-new-menu")
            .absolute()
            .right_0()
            .top(px(CONTROL_HEIGHT + 6.))
            .w(px(300.))
            .p(px(4.))
            .flex()
            .flex_col()
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE_RAISED)
            .shadow(popover_shadow())
            .occlude()
            .children(NEW_KINDS.iter().enumerate().map(|(i, k)| {
                let l = kind_looks(k.kind);
                let kind = k.kind;
                div()
                    .id(("agents-new-kind", i))
                    .flex()
                    .gap(px(10.))
                    .p(px(8.))
                    .rounded(px(RADIUS_MD))
                    .cursor_pointer()
                    .hover(|s| s.bg(SURFACE_HOVER))
                    .child(tile(l.icon, 28., RADIUS_MD, 14., l.tile_bg, l.tile_fg))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::MEDIUM).child(k.name))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .text_color(TEXT_MUTED)
                                    .child(k.desc),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_new_open(false, cx);
                        // A new pipeline starts in the editor (the mockup links to it).
                        if kind == Kind::Pipeline {
                            navigate(Screen::PipelineEditor, cx);
                        }
                    }))
            }))
    }
}
