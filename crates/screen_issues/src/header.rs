//! Page header (breadcrumb, open / waiting counts, "New issue") and the view tabs with the
//! steps of the selected issue's pipeline (else the default one).

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use pipeline::IssueStatus;
use ui::theme::*;
use ui::{Badge, Button, Status, TabItem, Tabs, Tone, icon};

use crate::{Card, IssuesScreen, View};

impl IssuesScreen {
    pub(crate) fn render_header(&self, cards: &[Card], cx: &mut Context<Self>) -> impl IntoElement {
        let repo: SharedString = self.pipes.read(cx).workspace_name().into();
        let waiting = cards
            .iter()
            .filter(|c| c.status == IssueStatus::Approval)
            .count();
        let failed = cards
            .iter()
            .filter(|c| c.status == IssueStatus::Failed)
            .count();

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
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(TEXT_HEADING))
                    .font_weight(FontWeight::MEDIUM)
                    .child(div().text_color(TEXT_MUTED).child(repo))
                    .child(icon("chevron-right", 12., TEXT_DISABLED))
                    .child("Issues"),
            )
            .child(Badge::new(format!("{} open", self.open_count(cx))))
            .when(waiting > 0, |d| {
                d.child(
                    Badge::new(format!("{waiting} need you"))
                        .tone(Tone::Warning)
                        .dot(Status::Waiting),
                )
            })
            .when(failed > 0, |d| {
                d.child(
                    Badge::new(format!("{failed} failed"))
                        .tone(Tone::Danger)
                        .dot(Status::Error),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("new-issue", "New issue")
                    .primary()
                    .icon("plus")
                    .on_click(cx.listener(|this, _, window, cx| this.open_new(window, cx))),
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

        // "Show the path": the steps of the pipeline in view.
        let (name, steps) = {
            let p = self.pipes.read(cx);
            let def = self
                .selected
                .as_ref()
                .and_then(|k| p.issue(k))
                .and_then(|i| p.pipeline_of(i))
                .or(p.default_pipeline());
            match def {
                Some(d) => (
                    d.name.clone(),
                    d.steps.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                ),
                None => (String::new(), Vec::new()),
            }
        };
        let path = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(6.))
            .py(px(6.))
            .font_family(FONT_MONO)
            .text_size(px(TEXT_MONO_SM))
            .text_color(TEXT_MUTED)
            .when(!name.is_empty(), |d| {
                d.child(
                    div()
                        .font_family(FONT_SANS)
                        .text_size(px(12.))
                        .text_color(TEXT_SECONDARY)
                        .child(SharedString::from(format!("{name}:"))),
                )
            })
            .children(steps.into_iter().enumerate().map(|(i, step)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(i > 0, |d| {
                        d.child(div().text_color(TEXT_DISABLED).child("›"))
                    })
                    .child(SharedString::from(step))
            }));

        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_x(px(16.))
            .gap_y(px(8.))
            .px(px(SPACE_6))
            .border_b_1()
            .border_color(LINE)
            .child(tabs)
            .child(div().flex_1())
            .child(path)
    }
}
