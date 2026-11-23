//! Bottom row: "Spend per day" bar chart, "Spend by agent" bars and "Runtimes".

use gpui::{IntoElement, SharedString, div, prelude::*, px, relative};
use ui::theme::*;
use ui::{Status, icon, status_dot};

use crate::data::{Range, looks, money};
use crate::flex_item;
use crate::stats::{AgentSpend, DaySpend, Kpis, nice_ceil};
use crate::tooltip::tooltip;
use crate::view::{ellipsis, empty_state, heading, muted};

/// Plot height of the daily spend chart.
const PLOT_H: f32 = 140.;
/// Width of the y-axis label column and its gap to the plot.
const AXIS_W: f32 = 40.;
const AXIS_GAP: f32 = 8.;
/// Gap between bars and the plot's side padding.
const BAR_GAP: f32 = 10.;
const BAR_PAD: f32 = 6.;
const BAR_MAX_W: f32 = 28.;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

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

/// Axis label: `$0`, `$0.25`, `$2.5`, `$10`.
fn axis_money(v: f64) -> String {
    if v == 0. {
        "$0".into()
    } else if v < 1. {
        format!("${v:.2}")
    } else if v.fract() == 0. {
        format!("${v:.0}")
    } else {
        format!("${v:.1}")
    }
}

// ── Spend per day ───────────────────────────────────────────────────────

fn grid_line(top: f32) -> gpui::Div {
    div()
        .absolute()
        .top(px(top))
        .left_0()
        .right_0()
        .h(px(1.))
        .bg(LINE_SUBTLE)
}

pub(crate) fn spend_per_day(days: &[DaySpend], offset: i64) -> impl IntoElement {
    let last = days.len().saturating_sub(1);
    let max = days.iter().map(DaySpend::total).fold(0., f64::max);
    let top = nice_ceil(max);
    let label_of = |d: &DaySpend| {
        let date = ui::time::date_of(d.start, offset);
        (WEEKDAYS[date.weekday as usize % 7], date)
    };

    let axis_label = |text: String| {
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
        .when(top > 0., |d| {
            d.child(axis_label(axis_money(top)).top(px(-8.)))
                .child(axis_label(axis_money(top / 2.)).top(px(PLOT_H / 2. - 8.)))
        })
        .child(axis_label(axis_money(0.)).bottom(px(-8.)));

    let bars = days.iter().enumerate().map(|(i, d)| {
        let today = i == last;
        let v = d.total();
        let h = if top > 0. {
            (v / top).clamp(0., 1.) as f32 * PLOT_H
        } else {
            0.
        };
        let (weekday, date) = label_of(d);
        let tip = format!(
            "{weekday} {} {}: {}{} · runs {} · chats {}",
            date.day,
            ui::time::month_name(date.month)
                .get(..3)
                .unwrap_or_default(),
            money(v),
            if today { " so far" } else { "" },
            money(d.runs),
            money(d.chats),
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
            .when(v > 0. && (today || v == max), |d| {
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
        .when(top > 0., |d| {
            d.child(grid_line(0.)).child(grid_line(PLOT_H / 2.))
        })
        .when(top == 0., |d| {
            d.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(muted("No spend in the last 7 days")),
            )
        })
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
        .children(days.iter().map(|d| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_center()
                .child(label_of(d).0)
        }));

    let week: f64 = days.iter().map(DaySpend::total).sum();
    let today = days.get(last).map_or(0., DaySpend::total);
    chart_card(1.4, 380., 10.)
        .child(card_head(
            "Spend per day",
            format!("last 7 days · {}", money(week)),
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
            "Today so far: {}. Hover a bar for the day total.",
            money(today)
        )))
}

// ── Spend by agent ──────────────────────────────────────────────────────

pub(crate) fn spend_by_agent(by: &[AgentSpend], range: Range) -> impl IntoElement {
    let max = by.iter().map(|r| r.cost).fold(0., f64::max);
    let rows = by.iter().enumerate().map(|(i, row)| {
        let (icon_name, color) = match row.kind {
            Some(kind) => {
                let l = looks(kind);
                (l.icon, l.fg)
            }
            None => ("more", TEXT_MUTED),
        };
        let w = if max > 0. {
            (row.cost / max * 100.).round().max(2.) / 100.
        } else {
            0.
        };
        div()
            .id(("home-by-agent", i))
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(24.))
            .tooltip(tooltip(format!(
                "{}: {} {}",
                row.name,
                money(row.cost),
                range.sentence()
            )))
            .child(
                div()
                    .w(px(112.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon(icon_name, 12., color))
                    .child(
                        ellipsis(row.name.clone(), 18.)
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
                            .w(relative(w as f32))
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
                    .child(money(row.cost)),
            )
    });
    chart_card(1., 320., 8.)
        .child(card_head("Spend by agent", range.sentence()))
        .when(by.is_empty(), |d| {
            d.child(empty_state(
                "No spend",
                "Pipeline agents and chats with a known price show up here.",
            ))
        })
        .children(rows)
}

// ── Runtimes ────────────────────────────────────────────────────────────

/// The local machine: every pipeline worker and chat runs here.
pub(crate) fn runtimes(host: &SharedString, k: &Kpis) -> impl IntoElement {
    let busy = k.working() > 0;
    let line = |label: &'static str, n: usize| {
        div()
            .flex()
            .items_center()
            .text_size(px(TEXT_LABEL))
            .child(div().flex_1().text_color(TEXT_SECONDARY).child(label))
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_MONO))
                    .text_color(if n > 0 { TEXT } else { TEXT_MUTED })
                    .child(n.to_string()),
            )
    };
    chart_card(1., 260., 12.).child(heading("Runtimes")).child(
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(status_dot(Status::Live, true))
                    .child(
                        ellipsis(host.clone(), 18.)
                            .flex_1()
                            .font_family(FONT_MONO)
                            .text_size(px(TEXT_MONO))
                            .line_height(px(18.)),
                    )
                    .child(muted(if busy {
                        format!("{} running", k.working())
                    } else {
                        "idle".to_string()
                    })),
            )
            .child(line("Pipeline workers", k.working_runs))
            .child(line("Chats", k.working_chats))
            .child(muted("Local machine: pipelines and chats run here.")),
    )
}
