//! The spend chart and "Spend by agent".

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};
use hub::api::{Period, SpendBucket, SpendByRunnable};
use ui::prelude::*;
use ui_rich::charts::{Bar, BarChart, ValueBars, ValueRow, axis_ticks, layout_bars};

use crate::blocks::{Block, error_view, probed, skeleton};
use crate::period;
use crate::view::{HomeView, SIDE_WIDTH};

/// Width the note under the chart wraps at: the column, its border and its padding off.
const NOTE_WIDTH: f32 = SIDE_WIDTH - 2. - 2. * SPACE_4;
/// Height of a row of `ValueBars`, for the probes laid over its rows.
const AGENT_ROW: f32 = 28.;
/// How many labels the x axis of a long day chart keeps.
const DAY_LABELS: usize = 6;

/// What the chart says about its bars, in the words of the mockup.
#[derive(Debug, PartialEq)]
pub(crate) struct ChartText {
    pub title: String,
    pub sub: String,
    pub note: String,
}

/// The weekday, `22 Nov` or `W47` a bar goes by.
fn bar_name(period: Period, bucket: &SpendBucket, offset: i32) -> String {
    let at = base::time::civil(bucket.start, offset);
    if period::weekly(period) {
        format!("W{}", base::time::iso_week(bucket.start, offset))
    } else if period == Period::Days30 {
        format!("{} {}", at.day, base::time::month_short(at.month))
    } else {
        base::time::weekday_short(at.weekday).to_string()
    }
}

/// The limit one bar is held against: the daily limit, a week's worth of it for weeks.
fn bar_limit(period: Period, daily: Option<f64>) -> Option<f64> {
    daily.map(|daily| if period::weekly(period) { daily * 7. } else { daily })
}

/// The bars of the chart: a label under the ones that fit, a tooltip on all.
pub(crate) fn bars(period: Period, buckets: &[SpendBucket], offset: i32) -> Vec<Bar> {
    let weekly = period::weekly(period);
    let last = buckets.len().saturating_sub(1);
    buckets
        .iter()
        .enumerate()
        .map(|(ix, bucket)| {
            let name = bar_name(period, bucket, offset);
            let label = match (ix == last, weekly) {
                (true, true) => "Now".to_string(),
                (true, false) => "Today".to_string(),
                (false, true) if ix % 3 == 0 => name.clone(),
                (false, true) => String::new(),
                (false, false) => name.clone(),
            };
            Bar::new(label, bucket.cost_usd).tooltip(format!("{name}: {}", base::fmt::cost(bucket.cost_usd)))
        })
        .collect()
}

/// Title, sub-line and note of the chart.
pub(crate) fn chart_text(period: Period, buckets: &[SpendBucket], daily: Option<f64>, offset: i32) -> ChartText {
    let weekly = period::weekly(period);
    let limit = bar_limit(period, daily);
    let n = buckets.len();
    let unit = if weekly { "week" } else { "day" };
    let mut sub = format!("last {n} {unit}s");
    if let Some(limit) = limit {
        sub.push_str(&format!(" · limit {} a {unit}", base::fmt::cost(limit)));
    }
    let so_far = buckets.last().map_or(0., |bucket| bucket.cost_usd);
    let current = if weekly { "This week" } else { "Today" };
    let mut note = format!("{current} so far: {}", base::fmt::cost(so_far));
    if let Some(limit) = limit {
        note.push_str(&format!(" of {}", base::fmt::cost(limit)));
    }
    note.push('.');
    if let Some(top) = buckets.iter().max_by(|a, b| a.cost_usd.total_cmp(&b.cost_usd)) {
        let scope = match period {
            Period::Today | Period::Days7 => "day this week".to_string(),
            Period::Days30 => "day of the last 30".to_string(),
            Period::Days90 => "week".to_string(),
        };
        note.push_str(&format!(
            " Highest {scope}: {}, {}.",
            bar_name(period, top, offset),
            base::fmt::cost(top.cost_usd)
        ));
    }
    if weekly && let Some(limit) = limit {
        let over = buckets.iter().filter(|bucket| bucket.cost_usd > limit).count() as u64;
        if over == 0 {
            note.push_str(" No week went over the limit.");
        } else {
            note.push_str(&format!(" {} went over the limit.", base::fmt::plural(over, "week", "weeks")));
        }
    }
    ChartText { title: format!("Spend per {unit}"), sub, note }
}

impl HomeView {
    /// "Spend per day" or "Spend per week".
    pub(crate) fn render_spend(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let period = self.period;
        let state = self.state(self.ops.read(cx).spend_buckets(period));
        let daily = self.ops.read(cx).settings().value().and_then(|settings| settings.daily_limit_usd);
        let offset = base::time::utc_offset(self.now());
        let (title, sub, body, note) = match state {
            Block::Ready(buckets) => {
                let text = chart_text(period, &buckets, daily, offset);
                let bars = bars(period, &buckets, offset);
                let limit = bar_limit(period, daily);
                let layout = layout_bars(&bars, limit, None, bars.len().checked_sub(1), None);
                let [top, middle, bottom] = axis_ticks(layout.max).map(base::fmt::cost_short);
                let chart = BarChart::new("home-spend-bars", bars)
                    .axis_labels(top, middle, bottom)
                    .when_some(limit, |chart, limit| chart.limit(limit))
                    .when(period == Period::Days30, |chart| chart.max_labels(DAY_LABELS));
                (text.title, text.sub, chart.into_any_element(), Some(text.note))
            }
            Block::Loading => {
                let unit = if period::weekly(period) { "week" } else { "day" };
                (
                    format!("Spend per {unit}"),
                    period::span(period).to_string(),
                    skeleton(NOTE_WIDTH, 120.).into_any_element(),
                    None,
                )
            }
            Block::Failed(message) => {
                let view = cx.entity().downgrade();
                let error = error_view("the spend", message, "spend", move |_, cx| {
                    view.update(cx, |view, cx| view.retry(cx)).ok();
                });
                ("Spend".to_string(), period::span(period).to_string(), error, None)
            }
        };
        let card = col()
            .flex_none()
            .w_full()
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .p(px(SPACE_4))
            .child(
                col()
                    .mb(px(SPACE_4))
                    .child(div().type_style(HEADING).child(title))
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(sub)),
            )
            .child(body)
            .when_some(note, |d, note| {
                d.child(div().mt(px(SPACE_3)).w(px(NOTE_WIDTH)).type_style(LABEL).text_color(TEXT_MUTED).child(note))
            });
        probed(card, "home-spend-chart").into_any_element()
    }

    /// "Spend by agent": the biggest spenders of the period, then the "Others" row.
    pub(crate) fn render_agents(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let period = self.period;
        let state = self.state(self.ops.read(cx).spend_by_runnable(period));
        let body = match state {
            Block::Ready(rows) if rows.is_empty() => {
                div().type_style(LABEL).text_color(TEXT_MUTED).child("Nothing has been spent yet.").into_any_element()
            }
            Block::Ready(rows) => agent_rows(&rows),
            Block::Loading => col()
                .children((0..5).map(|ix| {
                    row()
                        .h(px(AGENT_ROW))
                        .gap(px(SPACE_2_5))
                        .child(skeleton(80. + 6. * ix as f32, 12.))
                        .child(div().flex_1().min_w_0().h(px(6.)).rounded_full().bg(LINE))
                        .child(skeleton(48., 12.))
                }))
                .into_any_element(),
            Block::Failed(message) => {
                let view = cx.entity().downgrade();
                error_view("the spend by agent", message, "agents", move |_, cx| {
                    view.update(cx, |view, cx| view.retry(cx)).ok();
                })
            }
        };
        col()
            .flex_none()
            .w_full()
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .p(px(SPACE_4))
            .child(
                row()
                    .mb(px(SPACE_2))
                    .child(div().type_style(HEADING).child("Spend by agent"))
                    .child(grow())
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(period::span(period))),
            )
            .child(body)
            .into_any_element()
    }
}

/// The rows, each probed `home-agent-<n>`.
fn agent_rows(rows: &[SpendByRunnable]) -> AnyElement {
    ValueBars::new(rows.iter().enumerate().map(|(ix, row)| {
        let value = ValueRow::new(row.name.clone(), row.cost_usd).probe(format!("home-agent-{ix}"));
        if row.spawned { value.spawn() } else { value }
    }))
    .into_any_element()
}
