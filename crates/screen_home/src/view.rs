//! Header, KPI strip, "Working now" table and "Needs you" list.

use gpui::{
    Context, FontWeight, Hsla, IntoElement, SharedString, StyledText, TextRun, div, font,
    prelude::*, px,
};
use ui::nav::{self, Screen};
use ui::theme::*;
use ui::{Badge, Button, Stat, Status, Tone, icon, progress, status_dot};

use crate::data::{LEGEND, Range, count_label, looks, money};
use crate::rows::{LiveRow, NeedRow, RowState, age};
use crate::stats::Kpis;
use crate::{HomeScreen, flex_item, open};

/// Rows shown in "Needs you"; the rest are counted in the footer.
const NEEDS_SHOWN: usize = 6;

/// Fixed "Working now" columns after the two flexible ones: runtime, elapsed, cost, status.
const COL_RUNTIME: f32 = 90.;
const COL_ELAPSED: f32 = 56.;
const COL_COST: f32 = 60.;
const COL_STATUS: f32 = 120.;
const COL_GAP: f32 = 12.;

/// 14px/500 section title.
pub(crate) fn heading(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(TEXT_HEADING))
        .line_height(px(20.))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

/// 12px muted text.
pub(crate) fn muted(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(TEXT_LABEL))
        .text_color(TEXT_MUTED)
        .child(text.into())
}

/// Single-line text ending in `…` when too long.
///
/// `truncate()` (nowrap) caches its first measurement, which may come from an intrinsic
/// sizing pass at another width. A one-line clamp keeps normal wrapping, which is
/// re-measured whenever the width changes (same trick as `screen_issues::widgets`).
pub(crate) fn ellipsis(text: impl Into<SharedString>, line: f32) -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .h(px(line))
        .overflow_hidden()
        .text_ellipsis()
        .line_clamp(1)
        .child(text.into())
}

/// Footer link of a card ("All issues · 3 →").
fn footer_link(
    id: &'static str,
    text: impl Into<SharedString>,
    target: Screen,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(14.))
        .py(px(10.))
        .mt_auto()
        .text_size(px(TEXT_LABEL))
        .text_color(TEXT_MUTED)
        .cursor_pointer()
        .hover(|s| s.text_color(TEXT_SECONDARY))
        .on_click(move |_, _, cx| nav::navigate(target, cx))
        .child(text.into())
}

/// Centered muted note of an empty card ("Nothing needs you").
pub(crate) fn empty_state(title: &'static str, hint: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .px(px(14.))
        .py(px(28.))
        .child(
            div()
                .text_color(TEXT_SECONDARY)
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(muted(hint).text_center())
}

impl HomeScreen {
    // ── Header ──────────────────────────────────────────────────────────

    pub(crate) fn render_header(
        &self,
        now: u64,
        offset: i64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let date = ui::time::long_date(now, offset);
        let subtitle = if self.workspace.is_empty() {
            date
        } else {
            format!("{date} · {}", self.workspace)
        };
        let segments = Range::ALL.iter().map(|&range| {
            let on = self.range == range;
            div()
                .id(SharedString::from(format!("home-range-{}", range.label())))
                .flex()
                .items_center()
                .h(px(CONTROL_HEIGHT_SM))
                .px(px(12.))
                .rounded(px(RADIUS_MD))
                .text_size(px(TEXT_LABEL))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |d| d.bg(SURFACE_HOVER).text_color(TEXT))
                .when(!on, |d| {
                    d.text_color(TEXT_SECONDARY).hover(|s| s.text_color(TEXT))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.set_range(range, cx)))
                .child(range.label())
        });

        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_3))
            .min_h(px(HEADER_HEIGHT))
            .px(px(SPACE_6))
            .py(px(SPACE_2))
            .border_b_1()
            .border_color(LINE)
            .child(heading("Home"))
            .child(muted(subtitle))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded(px(RADIUS_LG))
                    .bg(BG_SIDEBAR)
                    .border_1()
                    .border_color(LINE)
                    .children(segments),
            )
            .child(
                Button::new("home-new-chat", "New chat")
                    .primary()
                    .icon("plus")
                    .on_click(|_, _, cx| nav::request_chat(nav::ChatRequest::New, cx)),
            )
    }

    // ── KPI strip ───────────────────────────────────────────────────────

    pub(crate) fn render_kpis(&self, k: &Kpis, now: u64) -> impl IntoElement {
        let working_sub = match (k.working_runs, k.working_chats) {
            (0, 0) => "nothing running".to_string(),
            (runs, 0) => count_label(runs, "pipeline", "pipelines"),
            (0, chats) => count_label(chats, "chat", "chats"),
            (runs, chats) => format!(
                "{} · {}",
                count_label(runs, "pipeline", "pipelines"),
                count_label(chats, "chat", "chats")
            ),
        };
        let waiting_sub = match k.oldest_wait {
            Some(t) if k.waiting > 0 => format!("oldest {}", age(t, now)),
            _ if k.waiting > 0 => "waiting".to_string(),
            _ => "all clear".to_string(),
        };
        let runs_sub = match k.done_pct() {
            Some(p) => format!("{p}% finished"),
            None if k.runs > 0 => "none finished yet".to_string(),
            None => "no runs".to_string(),
        };

        let stats = [
            Stat::new("Working now", k.working().to_string()).sub(working_sub),
            Stat::new("Waiting for you", k.waiting.to_string())
                .sub(waiting_sub)
                .tone(if k.waiting > 0 {
                    Tone::Warning
                } else {
                    Tone::Neutral
                }),
            Stat::new("Runs", k.runs.to_string()).sub(runs_sub),
            Stat::new("Spend", money(k.spend())).sub(self.range.sentence()),
            Stat::new("Shipped", k.shipped.to_string()).sub(if k.shipped == 1 {
                "pull request"
            } else {
                "pull requests"
            }),
        ];
        let items = stats.into_iter().enumerate().map(|(i, stat)| {
            flex_item(1., 160.)
                .px(px(20.))
                .border_l_1()
                .border_color(if i == 0 {
                    gpui::transparent_black()
                } else {
                    LINE
                })
                .child(stat.large())
        });
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .gap_y(px(SPACE_4))
            .py(px(14.))
            .px(px(4.))
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .children(items)
    }
}

// ── Working now ─────────────────────────────────────────────────────────

pub(crate) fn working_now(live: &[LiveRow], issues: usize) -> impl IntoElement {
    let legend = LEGEND.iter().map(|&(kind, one, many)| {
        let l = looks(kind);
        let n = live.iter().filter(|r| r.kind == kind).count();
        let label = if n == 1 { one } else { many };
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(TEXT_LABEL))
            .text_color(TEXT_SECONDARY)
            .child(
                div()
                    .size(px(18.))
                    .flex_none()
                    .rounded(px(RADIUS_SM))
                    .bg(l.bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(l.icon, 12., l.fg)),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .child(n.to_string()),
            )
            .child(label)
    });

    let head = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(12.))
        .gap_y(px(8.))
        .px(px(14.))
        .py(px(12.))
        .border_b_1()
        .border_color(LINE)
        .child(heading("Working now"))
        .child(div().flex_1())
        .children(legend);

    flex_item(999., 560.)
        .flex()
        .flex_col()
        .bg(SURFACE)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
        .child(head)
        .when(live.is_empty(), |d| {
            d.child(empty_state(
                "Nothing is running",
                "Start a pipeline on the issue board or open a chat.",
            ))
        })
        .when(!live.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(620.))
                    .children(live.iter().enumerate().map(|(i, row)| live_row(i, row))),
            )
        })
        .child(
            div()
                .mt_auto()
                .border_t_1()
                .border_color(LINE)
                .child(footer_link(
                    "home-all-issues",
                    format!("All issues · {issues} →"),
                    Screen::Issues,
                )),
        )
}

fn live_row(index: usize, row: &LiveRow) -> impl IntoElement {
    let l = looks(row.kind);
    let tile = div()
        .size(px(24.))
        .flex_none()
        .rounded(px(RADIUS_MD))
        .bg(l.bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(l.icon, 12., l.fg));

    let mono = |text: SharedString, w: f32, color: Hsla, right: bool| {
        div()
            .w(px(w))
            .flex_none()
            .font_family(FONT_MONO)
            .text_size(px(TEXT_MONO))
            .text_color(color)
            .when(right, |d| d.flex().justify_end())
            .child(text)
    };

    let target = row.target.clone();
    div()
        .id(("home-live", index))
        .flex()
        .items_center()
        .gap(px(COL_GAP))
        .min_h(px(44.))
        .px(px(14.))
        .py(px(6.))
        .border_t_1()
        .border_color(LINE_SUBTLE)
        .cursor_pointer()
        .hover(|s| s.bg(SURFACE_RAISED))
        .on_click(move |_, _, cx| open(&target, cx))
        .child(
            flex_item(1.3, 0.)
                .flex()
                .items_center()
                .gap(px(8.))
                .child(tile)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            ellipsis(row.name.clone(), 20.)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT),
                        )
                        .child(
                            ellipsis(row.sub.clone(), 16.)
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(TEXT_MUTED),
                        ),
                ),
        )
        .child(
            flex_item(1.5, 0.)
                .flex()
                .flex_col()
                .child(ellipsis(row.what.clone(), 20.).text_color(TEXT_SECONDARY))
                .child(
                    ellipsis(row.reference.clone(), 16.)
                        .font_family(FONT_MONO)
                        .text_size(px(TEXT_MONO_SM))
                        .line_height(px(16.))
                        .text_color(TEXT_MUTED),
                ),
        )
        .child(mono(row.runtime.clone(), COL_RUNTIME, TEXT_MUTED, false))
        .child(mono(row.elapsed.clone(), COL_ELAPSED, TEXT_MUTED, true))
        .child(mono(row.cost.clone(), COL_COST, TEXT, true))
        .child(status_cell(row))
}

/// Status column: dot + text, and a progress bar while running with known progress.
fn status_cell(row: &LiveRow) -> impl IntoElement {
    let (dot, text, color, fraction): (Status, SharedString, Hsla, Option<f32>) = match &row.state {
        RowState::Waiting(text) => (Status::Waiting, text.clone(), WARNING, None),
        RowState::Running(text, fraction) => {
            (Status::Running, text.clone(), TEXT_SECONDARY, *fraction)
        }
    };
    div()
        .w(px(COL_STATUS))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(6.))
                .text_size(px(TEXT_LABEL))
                .text_color(color)
                .child(div().mt(px(7.)).child(status_dot(dot, true)))
                .child(div().w(px(COL_STATUS - 12.)).child(text)),
        )
        .when_some(fraction, |d, f| d.child(progress(f, ACCENT)))
}

// ── Needs you ───────────────────────────────────────────────────────────

pub(crate) fn needs_you(needs: &[NeedRow]) -> impl IntoElement {
    // `flex: 1 1 320px` next to the 999-grow "Working now": 320px unless wrapped.
    flex_item(1., 320.)
        .flex()
        .flex_col()
        .bg(SURFACE)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(14.))
                .py(px(12.))
                .border_b_1()
                .border_color(LINE)
                .child(heading("Needs you"))
                .child(Badge::new(needs.len().to_string()).tone(Tone::Count)),
        )
        .when(needs.is_empty(), |d| {
            d.child(empty_state(
                "Nothing needs you",
                "Approvals, questions and reports show up here.",
            ))
        })
        .children(
            needs
                .iter()
                .take(NEEDS_SHOWN)
                .enumerate()
                .map(|(i, n)| need_row(i, n)),
        )
        .child(footer_link(
            "home-open-inbox",
            match needs.len().saturating_sub(NEEDS_SHOWN) {
                0 => "Open inbox →".to_string(),
                more => format!("{more} more · Open inbox →"),
            },
            Screen::Inbox,
        ))
}

fn need_row(index: usize, need: &NeedRow) -> impl IntoElement {
    let target = need.target.clone();
    div()
        .id(("home-need", index))
        .flex()
        .gap(px(10.))
        .px(px(14.))
        .py(px(10.))
        .border_b_1()
        .border_color(LINE_SUBTLE)
        .cursor_pointer()
        .hover(|s| s.bg(SURFACE_RAISED))
        .on_click(move |_, _, cx| open(&target, cx))
        .child(
            div()
                .mt(px(6.))
                .size(px(8.))
                .flex_none()
                .rounded_full()
                .bg(WARNING),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(need.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(TEXT_LABEL))
                        .line_height(px(18.))
                        .text_color(TEXT_MUTED)
                        .child(meta_line(need)),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .text_size(px(TEXT_LABEL))
                .text_color(ACCENT_TEXT)
                .whitespace_nowrap()
                .child(need.action.clone()),
        )
}

/// `who · <mono ref> · age` as one wrapping text with a monospace run.
fn meta_line(need: &NeedRow) -> StyledText {
    let mut parts = vec![(format!("{} · ", need.who), FONT_SANS)];
    if !need.reference.is_empty() {
        parts.push((need.reference.to_string(), FONT_MONO));
        parts.push((" · ".to_string(), FONT_SANS));
    }
    parts.push((need.age.to_string(), FONT_SANS));
    let text: String = parts.iter().map(|(s, _)| s.as_str()).collect();
    let runs = parts
        .iter()
        .map(|(s, family)| TextRun {
            len: s.len(),
            font: font(*family),
            color: TEXT_MUTED,
            background_color: None,
            underline: None,
            strikethrough: None,
        })
        .collect();
    StyledText::new(text).with_runs(runs)
}
