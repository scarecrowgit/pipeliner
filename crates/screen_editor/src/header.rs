//! Page header (breadcrumb, version badges, run/deploy buttons) and the canvas toolbar.

use gpui::{Context, FontWeight, IntoElement, div, prelude::*, px};
use ui::{Badge, Button, Status, Tone, icon, theme::*};

use crate::PipelineEditorScreen;
use crate::data::{ADD_BUTTONS, LAST_STEP};

impl PipelineEditorScreen {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let run_label = if self.running {
            "Running…"
        } else if self.step >= LAST_STEP {
            "Run again"
        } else {
            "Test run"
        };
        div()
            .flex()
            .flex_wrap()
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .id("crumb-pipelines")
                            .text_color(TEXT_MUTED)
                            .cursor_pointer()
                            .hover(|s| s.text_color(TEXT))
                            .on_click(|_, _, cx| ui::nav::navigate(ui::nav::Screen::Agents, cx))
                            .child("Pipelines"),
                    )
                    .child(icon("chevron-right", 12., TEXT_DISABLED))
                    .child(self.pipeline_name.clone()),
            )
            .child(Badge::new("Draft v15").tone(Tone::Accent))
            .child(Badge::new("v14 live").dot(Status::Live))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child("2 changes since v14"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        Button::new("replay", "Replay last 100")
                            .ghost()
                            .icon("replay"),
                    )
                    .child(
                        Button::new("test-run", run_label)
                            .icon("play")
                            .disabled(self.running)
                            .on_click(cx.listener(|this, _, _, cx| this.run(cx))),
                    )
                    .child(Button::new("deploy", "Deploy v15").primary()),
            )
    }

    pub(crate) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let zoom_pct = format!("{}%", (self.zoom * 100.).round() as i32);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .border_b_1()
            .border_color(LINE)
            .bg(BG)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .mr(px(4.))
                    .child("Add"),
            )
            .children(ADD_BUTTONS.iter().map(|&(kind, icon_name, label)| {
                Button::new(("add", kind as usize), label)
                    .small()
                    .icon(icon_name)
                    .on_click(cx.listener(move |this, _, _, cx| this.add_node(kind, cx)))
            }))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child("Drag a node by its title · drag from a port to connect"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        Button::new("zoom-out", "−")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.step_zoom(-0.1, cx))),
                    )
                    .child(
                        div()
                            .min_w(px(44.))
                            .flex()
                            .justify_center()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(TEXT_SECONDARY)
                            .child(zoom_pct),
                    )
                    .child(
                        Button::new("zoom-in", "+")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.step_zoom(0.1, cx))),
                    )
                    .child(
                        Button::new("fit", "Fit")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                    ),
            )
    }
}
