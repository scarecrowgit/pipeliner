//! List view: the same issues as a Graphite table.

use gpui::{AnyElement, Context, Hsla, IntoElement, SharedString, div, prelude::*, px};
use ui::theme::*;
use ui::{motion, status_dot};

use crate::look::column_dot;
use crate::widgets::{Ellipsis, status_line};
use crate::{Card, IssuesScreen};

/// Column widths: fixed px, or a flex share of the remaining width.
#[derive(Clone, Copy)]
enum Width {
    Fixed(f32),
    Flex(f32),
}

const COLUMNS: [(&str, Width); 6] = [
    ("Key", Width::Fixed(96.)),
    ("Title", Width::Flex(2.4)),
    ("Kind", Width::Fixed(96.)),
    ("Status", Width::Fixed(148.)),
    ("Pipeline", Width::Flex(1.6)),
    ("Time", Width::Fixed(80.)),
];

fn cell(width: Width, content: impl IntoElement) -> gpui::Div {
    let d = div().px(px(16.)).py(px(10.)).min_w_0().ellipsis();
    match width {
        Width::Fixed(w) => d.w(px(w)).flex_none(),
        Width::Flex(f) => d
            .flex_grow()
            .flex_shrink()
            .flex_basis(px(0.))
            .min_w(px(80. * f)),
    }
    .child(content)
}

fn text(t: impl Into<SharedString>, color: Hsla) -> gpui::Div {
    div().text_color(color).child(t.into())
}

impl IssuesScreen {
    pub(crate) fn render_list(&self, cards: &[Card], cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = cards
            .iter()
            .map(|c| self.render_row(c, cx).into_any_element())
            .collect();

        div().id("list-scroll").size_full().overflow_scroll().child(
            div()
                .pt(px(SPACE_4))
                .px(px(SPACE_6))
                .pb(px(SPACE_6))
                .min_w(px(760. + 48.))
                .child(
                    div()
                        .rounded(px(RADIUS_XL))
                        .border_1()
                        .border_color(LINE)
                        .bg(SURFACE)
                        .overflow_hidden()
                        .child(div().flex().children(COLUMNS.iter().map(|(name, w)| {
                            cell(*w, *name)
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .whitespace_nowrap()
                        })))
                        .children(rows),
                ),
        )
    }

    fn render_row(&self, card: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.as_ref() == Some(&card.key);
        let key = card.key.clone();
        let id = SharedString::from(format!("row-{}", card.key));
        let [k, title, kind, status, line, time] = COLUMNS.map(|(_, w)| w);

        let row = div()
            .id(id.clone())
            .flex()
            .items_center()
            .h(px(ROW_HEIGHT))
            .border_t_1()
            .border_color(LINE)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.select(key.clone(), window, cx)))
            .child(cell(
                k,
                text(card.key.clone(), TEXT_MUTED)
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO)),
            ))
            .child(cell(title, text(card.title.clone(), TEXT)))
            .child(cell(kind, text(card.kind.clone(), TEXT_SECONDARY)))
            .child(cell(
                status,
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(status_dot(column_dot(card.status), true))
                    .child(text(card.status.label(), TEXT)),
            ))
            .child(cell(line, status_line(&card.line)))
            .child(cell(
                time,
                text(
                    card.elapsed.clone().unwrap_or_else(|| "—".into()),
                    TEXT_MUTED,
                )
                .font_family(FONT_MONO)
                .text_size(px(TEXT_MONO)),
            ));
        motion::fade_row(row, id, selected, SURFACE_RAISED)
    }
}
