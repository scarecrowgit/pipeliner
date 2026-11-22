//! Page header (breadcrumb, open count, actions) and the view tabs + runner filter chips.

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use ui::theme::*;
use ui::{Badge, Button, TabItem, Tabs, icon};

use crate::data::{Column, Filter};
use crate::{IssuesScreen, View};

impl IssuesScreen {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(div().text_color(TEXT_MUTED).child("Payments"))
                    .child(icon("chevron-right", 12., TEXT_DISABLED))
                    .child("Issues"),
            )
            .child(Badge::new(format!("{} open", self.open_count())))
            .child(div().flex_1())
            .child(
                Button::new("autopilots", "Autopilots · 2")
                    .ghost()
                    .icon("replay"),
            )
            .child(
                Button::new("new-issue", "New issue")
                    .primary()
                    .icon("plus")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_issue(Column::Backlog, window, cx)
                    })),
            )
    }

    pub(crate) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let tabs = Tabs::new(
            "issues-view",
            vec![TabItem::new("Board"), TabItem::new("List")],
            match self.view {
                View::Board => 0,
                View::List => 1,
            },
        )
        .on_select(move |ix, _, cx| {
            let view = if ix == 0 { View::Board } else { View::List };
            weak.update(cx, |this, cx| this.set_view(view, cx)).ok();
        });

        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_x(px(16.))
            .gap_y(px(8.))
            .pl(px(24.))
            .pr(px(24.))
            .border_b_1()
            .border_color(LINE)
            .child(tabs)
            .child(div().flex_1())
            .child(
                div().flex().flex_wrap().gap(px(4.)).py(px(6.)).children(
                    Filter::ALL
                        .into_iter()
                        .map(|f| self.render_filter_chip(f, cx))
                        .collect::<Vec<_>>(),
                ),
            )
    }

    fn render_filter_chip(
        &self,
        filter: Filter,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let on = self.filter == filter;
        div()
            .id(SharedString::from(format!("filter-{}", filter.label())))
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(CONTROL_HEIGHT_SM))
            .px(px(10.))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(if on {
                LINE_STRONG
            } else {
                gpui::transparent_black()
            })
            .bg(if on {
                SURFACE_HOVER
            } else {
                gpui::transparent_black()
            })
            .text_color(if on { TEXT } else { TEXT_SECONDARY })
            .text_size(px(12.))
            .whitespace_nowrap()
            .cursor_pointer()
            .when(!on, |d| d.hover(|s| s.bg(SURFACE_RAISED)))
            .child(filter.label())
            .child(
                div()
                    .text_color(TEXT_MUTED)
                    .child(SharedString::from(self.filter_count(filter).to_string())),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
    }
}
