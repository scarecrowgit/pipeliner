//! List view: the same (filtered) issues as a Graphite table.

use gpui::{AnyElement, Context, Hsla, IntoElement, SharedString, div, prelude::*, px};
use ui::theme::*;

use crate::IssuesScreen;
use crate::data::Issue;
use crate::widgets::Ellipsis;

/// Column widths: fixed px, or a flex share of the remaining width.
#[derive(Clone, Copy)]
enum Width {
    Fixed(f32),
    Flex(f32),
}

const COLUMNS: [(&str, Width); 6] = [
    ("Key", Width::Fixed(112.)),
    ("Title", Width::Flex(2.)),
    ("Status", Width::Fixed(112.)),
    ("Runs with", Width::Flex(1.4)),
    ("Run", Width::Flex(1.6)),
    ("Priority", Width::Fixed(96.)),
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
    pub(crate) fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = self
            .visible()
            .map(|i| self.render_row(i, cx).into_any_element())
            .collect();

        div().id("list-scroll").size_full().overflow_scroll().child(
            div()
                .pt(px(16.))
                .px(px(24.))
                .pb(px(24.))
                .min_w(px(720. + 48.))
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

    fn render_row(&self, issue: &Issue, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.as_ref() == Some(&issue.key);
        let key = issue.key.clone();
        let runner = issue
            .runner()
            .map(|r| format!("{} · {}", r.kind.label(), r.name))
            .unwrap_or_else(|| "—".into());
        let run = issue
            .run
            .as_ref()
            .map(|r| r.text.clone())
            .unwrap_or_else(|| "—".into());
        let [k, title, status, runs, run_w, prio] = COLUMNS.map(|(_, w)| w);

        div()
            .id(SharedString::from(format!("row-{}", issue.key)))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(LINE)
            .when(selected, |d| d.bg(SURFACE_RAISED))
            .hover(|s| s.bg(SURFACE_RAISED))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.select(key.clone(), window, cx)))
            .child(cell(
                k,
                text(issue.key.clone(), TEXT_MUTED)
                    .font_family(FONT_MONO)
                    .text_size(px(12.)),
            ))
            .child(cell(title, text(issue.title.clone(), TEXT)))
            .child(cell(status, text(issue.status.name(), TEXT)))
            .child(cell(runs, text(runner, TEXT)))
            .child(cell(run_w, text(run, TEXT_MUTED)))
            .child(cell(prio, text(issue.prio.label(), TEXT)))
    }
}
