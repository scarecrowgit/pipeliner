//! Bottom row: "Spend per day" bar chart, "Spend by agent" bars and "Runtimes".

use gpui::{IntoElement, SharedString, div, prelude::*, px, relative};
use ui::theme::*;
use ui::{Status, icon, progress, status_dot};

use crate::data::{DAY_LIMIT, DAYS, RUNTIMES, Range, looks};
use crate::flex_item;
use crate::tooltip::tooltip;
use crate::view::{ellipsis, heading, muted};

/// Plot height of the daily spend chart.
const PLOT_H: f32 = 140.;
/// Width of the y-axis label column and its gap to the plot.
const AXIS_W: f32 = 36.;
const AXIS_GAP: f32 = 8.;
/// Gap between bars and the plot's side padding.
const BAR_GAP: f32 = 10.;
const BAR_PAD: f32 = 6.;
const BAR_MAX_W: f32 = 28.;

/// Card shell of the bottom row: `flex: <grow> 1 <basis>px`, 12/14/14 padding.
fn chart_card(grow: f32, basis: f32, gap: f32) -> gpui::Div {
    flex_item(grow, basis)
        .flex()
        .flex_col()
        .gap(px(gap))
        .pt(px(12.))
        .px(px(14.))
        .pb(px(14.))
        .bg(SURFACE)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
}

/// Title with a muted note on the same baseline.
fn card_head(title: &'static str, note: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(8.))
        .child(heading(title))
        .child(muted(note))
}

fn money(v: f32) -> String {
    format!("${v:.2}")
}

// ── Spend per day ───────────────────────────────────────────────────────

/// 1px dashed limit line along the top of the plot (3px dashes, clipped to the width).
fn dashed_line() -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(1.))
        .overflow_hidden()
        .flex()
        .gap(px(3.))
        .children((0..160).map(|_| div().w(px(3.)).h_full().flex_none().bg(LINE_STRONG)))
}

pub(crate) fn spend_per_day() -> impl IntoElement {
    let last = DAYS.len() - 1;
    let max = DAYS.iter().map(|d| d.1).fold(0., f32::max);

    let axis_label = |text: &'static str| {
        div()
            .absolute()
            .right_0()
            .h(px(16.))
            .line_height(px(16.))
            .child(text)
    };
    let axis = div()
        .relative()
        .w(px(AXIS_W))
        .h(px(PLOT_H))
        .flex_none()
        .font_family(FONT_MONO)
        .text_size(px(TEXT_MONO_SM))
        .text_color(TEXT_MUTED)
        .child(axis_label("$10").top(px(-8.)))
        .child(axis_label("$5").top(px(PLOT_H / 2. - 8.)))
        .child(axis_label("$0").bottom(px(-8.)));

    let bars = DAYS.iter().enumerate().map(|(i, &(day, v))| {
        let today = i == last;
        let h = (v / DAY_LIMIT).clamp(0., 1.) * PLOT_H;
        let tip = format!(
            "{day}: {} of {}{}",
            money(v),
            money(DAY_LIMIT),
            if today { " so far" } else { "" }
        );
        div()
            .id(("home-day", i))
            .group("home-day")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_end()
            .justify_center()
            .tooltip(tooltip(tip))
            .child(
                div()
                    .w_full()
                    .max_w(px(BAR_MAX_W))
                    .h(px(h))
                    .rounded_t(px(RADIUS_SM))
                    .bg(if today { ACCENT_RING } else { ACCENT })
                    .group_hover("home-day", |s| s.bg(ACCENT_TEXT)),
            )
            .when(today || v == max, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(h + 4.))
                        .flex()
                        .justify_center()
                        .font_family(FONT_MONO)
                        .text_size(px(TEXT_MONO_SM))
                        .line_height(px(16.))
                        .text_color(TEXT_SECONDARY)
                        .child(money(v)),
                )
            })
    });

    let plot = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(PLOT_H))
        .border_b_1()
        .border_color(LINE_STRONG)
        // Limit line (dashed) and the $5 grid line.
        .child(dashed_line())
        .child(
            div()
                .absolute()
                .top(px(PLOT_H / 2.))
                .left_0()
                .right_0()
                .h(px(1.))
                .bg(LINE_SUBTLE),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_end()
                .gap(px(BAR_GAP))
                .px(px(BAR_PAD))
                .children(bars),
        );

    let day_labels = div()
        .flex()
        .gap(px(BAR_GAP))
        .px(px(BAR_PAD))
        .ml(px(AXIS_W + AXIS_GAP))
        .font_family(FONT_MONO)
        .text_size(px(TEXT_MONO_SM))
        .line_height(px(16.))
        .text_color(TEXT_MUTED)
        .children(
            DAYS.iter()
                .map(|&(day, _)| div().flex_1().min_w_0().flex().justify_center().child(day)),
        );

    chart_card(1.4, 380., 10.)
        .child(card_head(
            "Spend per day",
            format!("last 7 days · limit {} a day", money(DAY_LIMIT)),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(AXIS_GAP))
                .child(div().flex().gap(px(AXIS_GAP)).child(axis).child(plot))
                .child(day_labels),
        )
        .child(muted(format!(
            "Today so far is lighter: {} of {}. Hover a bar for the day total.",
            money(DAYS[last].1),
            money(DAY_LIMIT)
        )))
}

// ── Spend by agent ──────────────────────────────────────────────────────

pub(crate) fn spend_by_agent(range: Range) -> impl IntoElement {
    let by = range.by_agent();
    let max = by[0].2;
    let rows = by.iter().enumerate().map(|(i, &(name, kind, v))| {
        let l = looks(kind);
        let w = (v / max * 100.).round().max(2.) / 100.;
        div()
            .id(("home-by-agent", i))
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(24.))
            .tooltip(tooltip(format!(
                "{name}: {} {}",
                money(v),
                range.sentence()
            )))
            .child(
                div()
                    .w(px(112.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon(l.icon, 12., l.fg))
                    .child(
                        ellipsis(name, 18.)
                            .flex_1()
                            .text_size(px(TEXT_LABEL))
                            .line_height(px(18.))
                            .text_color(TEXT_SECONDARY),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(8.))
                    .rounded_r(px(RADIUS_SM))
                    .bg(LINE_SUBTLE)
                    .child(
                        div()
                            .h_full()
                            .w(relative(w))
                            .rounded_r(px(RADIUS_SM))
                            .bg(ACCENT),
                    ),
            )
            .child(
                div()
                    .w(px(60.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO))
                    .child(money(v)),
            )
    });
    chart_card(1., 320., 8.)
        .child(card_head("Spend by agent", range.sentence()))
        .children(rows)
}

// ── Runtimes ────────────────────────────────────────────────────────────

pub(crate) fn runtimes() -> impl IntoElement {
    let items = RUNTIMES.iter().map(|rt| {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(status_dot(
                        if rt.online {
                            Status::Live
                        } else {
                            Status::Draft
                        },
                        true,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .font_family(FONT_MONO)
                            .text_size(px(TEXT_MONO))
                            .child(rt.name),
                    )
                    .child(muted(rt.usage)),
            )
            .child(progress(rt.used as f32 / rt.max as f32, ACCENT))
            .child(muted(rt.note))
    });
    chart_card(1., 260., 12.)
        .child(heading("Runtimes"))
        .children(items)
}
