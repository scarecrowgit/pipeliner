//! Page header: breadcrumb with the session title, repo/branch pill, New session.

use gpui::{Context, FontWeight, IntoElement, div, prelude::*, px};
use ui::{Button, icon, theme::*};

use crate::ChatScreen;

impl ChatScreen {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let breadcrumb = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .text_size(px(14.))
            .line_height(px(20.))
            .font_weight(FontWeight::MEDIUM)
            .child(div().flex_none().text_color(TEXT_MUTED).child("Chat"))
            .child(icon("chevron-right", 12., TEXT_DISABLED))
            .child(div().min_w_0().truncate().child(self.title()));

        let pill = match &self.repo {
            Some((name, Some(branch))) => format!("{name} · {branch}"),
            Some((name, None)) => name.clone(),
            None => self.repo_name(),
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .min_h(px(HEADER_HEIGHT))
            .px(px(24.))
            .py(px(8.))
            .border_b_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(RADIUS_MD))
                    .bg(ACCENT_SOFT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("pipelines", 16., ACCENT_TEXT)),
            )
            .child(breadcrumb)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(22.))
                    .px(px(8.))
                    .rounded_full()
                    .border_1()
                    .border_color(LINE_STRONG)
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .text_color(TEXT_SECONDARY)
                    .whitespace_nowrap()
                    .child(pill),
            )
            .child(div().flex_1())
            .child(
                Button::new("new-session", "New session")
                    .icon("plus")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_session(cx);
                        this.focus_input(window, cx);
                    })),
            )
    }
}
