//! Board view: horizontally scrolling columns, one per issue status, and the empty state.

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, SharedString, Stateful, div, prelude::*, px,
};
use pipeline::IssueStatus;
use ui::motion::{Look, fade};
use ui::theme::*;
use ui::{Badge, Button, icon, progress, status_dot};

use crate::look::{COLUMNS, column_dot};
use crate::widgets::{mono, small, status_line};
use crate::{Card, IssuesScreen};

const COLUMN_WIDTH: f32 = 252.;

impl IssuesScreen {
    pub(crate) fn render_board(&self, cards: &[Card], cx: &mut Context<Self>) -> impl IntoElement {
        let failed = cards.iter().any(|c| c.status == IssueStatus::Failed);
        let columns: Vec<IssueStatus> = COLUMNS
            .into_iter()
            .chain(failed.then_some(IssueStatus::Failed))
            .collect();

        div()
            .id("board-scroll")
            .size_full()
            .flex()
            .items_start()
            .overflow_scroll()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_start()
                    .gap(px(SPACE_3))
                    .pt(px(SPACE_4))
                    .px(px(SPACE_6))
                    .pb(px(SPACE_6))
                    .children(
                        columns
                            .into_iter()
                            .map(|c| self.render_column(c, cards, cx))
                            .collect::<Vec<_>>(),
                    ),
            )
    }

    fn render_column(&self, status: IssueStatus, cards: &[Card], cx: &mut Context<Self>) -> Div {
        let mine: Vec<&Card> = cards.iter().filter(|c| c.status == status).collect();
        let n = mine.len();

        div()
            .w(px(COLUMN_WIDTH))
            .flex_none()
            .min_h(px(480.))
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(4.))
                    .child(status_dot(column_dot(status), true))
                    .child(div().font_weight(FontWeight::MEDIUM).child(status.label()))
                    .child(
                        div()
                            .text_color(TEXT_MUTED)
                            .child(SharedString::from(n.to_string())),
                    )
                    .child(div().flex_1())
                    .when(status == IssueStatus::Approval && n > 0, |d| {
                        d.child(small(format!("{n} need you"), WARNING))
                    }),
            )
            .children(mine.into_iter().map(|c| self.render_card(c, cx)))
            .when(status == IssueStatus::Backlog, |d| {
                d.child(
                    div()
                        .id("board-add")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(CONTROL_HEIGHT_SM))
                        .px(px(8.))
                        .rounded(px(RADIUS_MD))
                        .text_color(TEXT_MUTED)
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|s| s.bg(SURFACE_RAISED).text_color(TEXT_SECONDARY))
                        .child(icon("plus", 12., TEXT_MUTED))
                        .child("New issue")
                        .on_click(cx.listener(|this, _, window, cx| this.open_new(window, cx))),
                )
            })
    }

    fn render_card(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected.as_ref() == Some(&card.key);
        let key = card.key.clone();
        let id = SharedString::from(format!("card-{}", card.key));
        let border = if selected { ACCENT_RING } else { LINE };
        let rest = Look::bg(SURFACE).border(border);
        let hover = Look::bg(SURFACE).border(if selected { ACCENT_RING } else { LINE_STRONG });

        let el: Stateful<Div> = div()
            .id(id.clone())
            .flex()
            .flex_col()
            .gap(px(8.))
            .w_full()
            .px(px(12.))
            .py(px(10.))
            .rounded(px(RADIUS_XL))
            .border_1()
            .when(selected, |d| d.shadow(selected_glow()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.select(key.clone(), window, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .w_full()
                    .child(mono(card.key.clone(), TEXT_MUTED))
                    .child(div().flex_1())
                    .when_some(card.elapsed.clone(), |d, e| d.child(mono(e, TEXT_MUTED))),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(18.))
                    .child(card.title.clone()),
            )
            .child(div().flex().child(Badge::new(card.kind.clone())))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .w_full()
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(LINE_SUBTLE)
                    .child(self.card_line(card, cx))
                    .when_some(card.progress, |d, f| d.child(progress(f, ACCENT))),
            );
        fade(el, id, rest, hover).into_any_element()
    }

    /// The run line; "PR open" is a link to the pull request.
    fn card_line(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let Some(url) = card.line.pr.clone() else {
            return status_line(&card.line).into_any_element();
        };
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .id(SharedString::from(format!("card-pr-{}", card.key)))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(ACCENT_TEXT)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(icon("git-branch", 12., ACCENT_TEXT))
                    .child("PR open")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&url);
                    })),
            )
            .into_any_element()
    }

    /// No issues yet: what the board is for and a way to start.
    pub(crate) fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let errors = self.pipes.read(cx).pipeline_errors.join("\n");
        let errors = (!errors.is_empty()).then_some(errors);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(SPACE_4))
            .px(px(SPACE_6))
            .child(
                div()
                    .size(px(40.))
                    .rounded(px(RADIUS_XL))
                    .bg(SURFACE_RAISED)
                    .border_1()
                    .border_color(LINE_STRONG)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("pipelines", 18., TEXT_SECONDARY)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .max_w(px(420.))
                    .child(
                        div()
                            .text_size(px(TEXT_HEADING))
                            .font_weight(FontWeight::MEDIUM)
                            .child("No issues yet"),
                    )
                    .child(div().text_center().text_color(TEXT_MUTED).child(
                        "Describe a change and pick a pipeline; its steps take it from \
                                 there and stop when one needs you.",
                    )),
            )
            .child(
                // The header's "New issue" is the page's one primary action.
                Button::new("empty-new-issue", "New issue")
                    .icon("plus")
                    .on_click(cx.listener(|this, _, window, cx| this.open_new(window, cx))),
            )
            .when_some(errors, |d, e| {
                d.child(div().max_w(px(520.)).text_center().child(small(
                    format!("Some pipeline files could not be read: {e}"),
                    WARNING,
                )))
            })
    }
}
