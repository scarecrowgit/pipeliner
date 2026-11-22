//! "Attach agent, orchestrator or pipeline" popover: search + grouped runner options.

use gpui::{AnyElement, Context, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use ui::theme::*;
use ui::{icon, icon_tile};

use crate::IssuesScreen;
use crate::data::{self, RunnerId, RunnerKind};
use crate::widgets::Ellipsis;

/// Group order and headings.
pub const GROUPS: [(RunnerKind, &str); 3] = [
    (RunnerKind::Orch, "Orchestrators"),
    (RunnerKind::Agent, "Agents"),
    (RunnerKind::Pipeline, "Pipelines"),
];

impl IssuesScreen {
    pub(crate) fn render_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.picker_matches(cx);
        let current = self.selected_issue().and_then(|i| i.runner);

        let mut options: Vec<AnyElement> = Vec::new();
        for (kind, heading) in GROUPS {
            let items: Vec<RunnerId> = matches
                .iter()
                .copied()
                .filter(|id| data::runner(*id).kind == kind)
                .collect();
            if items.is_empty() {
                continue;
            }
            options.push(
                div()
                    .pt(px(8.))
                    .px(px(8.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(heading)
                    .into_any_element(),
            );
            for id in items {
                options.push(
                    self.render_option(id, current == Some(id), cx)
                        .into_any_element(),
                );
            }
        }
        if options.is_empty() {
            options.push(
                div()
                    .px(px(8.))
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child("No matches")
                    .into_any_element(),
            );
        }

        div()
            .flex_none()
            .mt(px(12.))
            .mx(px(20.))
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(ACCENT_RING)
            .shadow(selected_glow())
            .bg(SURFACE)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(40.))
                    .pl(px(12.))
                    .pr(px(12.))
                    .border_b_1()
                    .border_color(LINE)
                    .text_color(TEXT)
                    .child(icon("search", 14., TEXT_MUTED))
                    .child(self.pick_input.clone()),
            )
            .child(
                div()
                    .id("picker-options")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .p(px(4.))
                    .children(options),
            )
    }

    fn render_option(&self, id: RunnerId, on: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let r = data::runner(id);
        div()
            .id(SharedString::from(format!("pick-{}", r.id)))
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .min_h(px(44.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(RADIUS_MD))
            .bg(if on {
                SURFACE_HOVER
            } else {
                gpui::transparent_black()
            })
            .hover(|s| s.bg(SURFACE_HOVER))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.attach_runner(id, window, cx)))
            .child(icon_tile(r.kind.icon(), r.kind.tone()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::MEDIUM).child(r.name))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .ellipsis()
                            .child(r.desc),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .whitespace_nowrap()
                    .text_color(if r.load == "blocked" {
                        WARNING
                    } else {
                        TEXT_MUTED
                    })
                    .child(r.load),
            )
    }
}
