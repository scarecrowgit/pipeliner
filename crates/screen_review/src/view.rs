//! Render parts of the Inbox: header, queue (approvals + reports), main column.

use gpui::{Context, FontWeight, IntoElement, div, prelude::*, px};
use ui::theme::*;
use ui::{Badge, Status, Tone, icon, kbd};

use crate::ReviewScreen;

/// Small caption above a group of queue rows.
fn section_label(text: &'static str) -> gpui::Div {
    div()
        .flex_none()
        .px(px(SPACE_2))
        .pt(px(SPACE_2))
        .pb(px(2.))
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(TEXT_MUTED)
        .child(text)
}

impl ReviewScreen {
    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let approvals = self.approvals(cx).len();
        let unread = agent_tools::inbox::unread(&self.reports);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(SPACE_3))
            .min_h(px(HEADER_HEIGHT))
            .px(px(SPACE_6))
            .py(px(SPACE_2))
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
                    .child(icon("inbox", 16., ACCENT_TEXT)),
            )
            .child(
                div()
                    .text_size(px(TEXT_HEADING))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Inbox"),
            )
            .when(approvals > 0, |d| {
                d.child(
                    Badge::new(format!(
                        "{approvals} to approve",
                    ))
                    .tone(Tone::Warning)
                    .dot(Status::Waiting),
                )
            })
            .when(unread > 0, |d| {
                d.child(Badge::new(format!("{unread} unread")).tone(Tone::Accent))
            })
    }

    pub(crate) fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let approvals = self.render_approval_rows(cx);
        let reports = self.render_report_rows(cx);
        let waiting = approvals.len() + agent_tools::inbox::unread(&self.reports);
        let empty = approvals.is_empty() && reports.is_empty();
        div()
            .id("review-queue")
            .w(px(300.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(SPACE_3))
            .py(px(SPACE_4))
            .bg(BG_SIDEBAR)
            .border_r_1()
            .border_color(LINE)
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(px(SPACE_2))
                    .pb(px(SPACE_2))
                    .child(
                        div()
                            .text_size(px(TEXT_HEADING))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Waiting for you"),
                    )
                    .when(waiting > 0, |d| {
                        d.child(Badge::new(waiting.to_string()).tone(Tone::Count))
                    }),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .px(px(SPACE_2))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(TEXT_MUTED)
                        .child(
                            "Nothing waits for you. Pipeline steps waiting for your approval and \
                             reports agents post when they finish (with their pull requests) \
                             land here.",
                        ),
                )
            })
            .when(!approvals.is_empty(), |d| {
                d.child(section_label("Waiting for approval")).children(approvals)
            })
            .when(!reports.is_empty(), |d| {
                d.child(section_label("From agents")).children(reports)
            })
            .child(
                div()
                    .mt_auto()
                    .pt(px(SPACE_3))
                    .px(px(SPACE_2))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(kbd("J"))
                    .child(kbd("K"))
                    .child("move"),
            )
    }

    pub(crate) fn render_main(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if let Some(key) = self.approval.clone() {
            self.render_approval(&key, cx)
        } else if let Some(r) = self.report() {
            self.render_report(r, cx)
        } else {
            div()
                .pt(px(SPACE_8))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(SPACE_2))
                .text_color(TEXT_MUTED)
                .child(icon("inbox", 24., TEXT_DISABLED))
                .child("Pick an item on the left.")
                .into_any_element()
        };
        div()
            .id("review-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(SPACE_6))
                    .w_full()
                    .max_w(px(1180.))
                    .child(content),
            )
    }
}
