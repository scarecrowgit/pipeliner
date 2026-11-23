//! The `.hbar` rows of Home's "Spend by agent": a name, a track with a bar and an amount.

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, SharedString, Window, div, prelude::*, px,
    relative,
};
use ui::layout::truncate_lh;
use ui::theme::*;

use super::scale::fraction;

/// Name column width.
const NAME_WIDTH: f32 = 112.0;
/// Amount column width.
const AMOUNT_WIDTH: f32 = 64.0;
/// Space between the three columns.
const COLUMN_GAP: f32 = 10.0;
/// Row and track heights: full size, then compact (popover).
const ROW_HEIGHT: f32 = 28.0;
const TRACK_HEIGHT: f32 = 6.0;
const COMPACT_ROW_HEIGHT: f32 = 24.0;
const COMPACT_TRACK_HEIGHT: f32 = 4.0;

/// One row: a name, a value and whether the row stands for something created by an agent.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueRow {
    name: SharedString,
    value: f64,
    amount: Option<SharedString>,
    spawn: bool,
}

impl ValueRow {
    /// A row called `name` worth `value`.
    pub fn new(name: impl Into<SharedString>, value: f64) -> Self {
        Self { name: name.into(), value, amount: None, spawn: false }
    }

    /// The amount text, instead of the default formatting of the value.
    pub fn amount(mut self, text: impl Into<SharedString>) -> Self {
        self.amount = Some(text.into());
        self
    }

    /// The `spawn` tone: the bar is violet.
    pub fn spawn(mut self) -> Self {
        self.spawn = true;
        self
    }
}

/// How full each bar is: each value relative to the largest, `0..=1`.
///
/// The largest value gives `1`. Zero, negative and non-finite values give `0`, and so does
/// everything when no value is positive.
pub fn value_fractions(values: &[f64]) -> Vec<f32> {
    let top = values.iter().copied().filter(|v| v.is_finite()).fold(0.0, f64::max);
    values.iter().map(|v| fraction(*v, top)).collect()
}

/// Rows of `name — bar — amount`, the bars relative to the largest value.
///
/// The name (112 px) truncates, the track fills the rest and the amount (64 px, mono) is right
/// aligned. [`compact`](Self::compact) is the popover variant: 24 px rows, 4 px track.
#[derive(IntoElement)]
pub struct ValueBars {
    rows: Vec<ValueRow>,
    compact: bool,
    format: fn(f64) -> String,
}

impl ValueBars {
    /// Bars for `rows`.
    pub fn new(rows: impl IntoIterator<Item = ValueRow>) -> Self {
        Self { rows: rows.into_iter().collect(), compact: false, format: base::fmt::cost }
    }

    /// 24 px rows with a 4 px track, for a popover.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// How an amount is written when the row has no text of its own (default: dollars).
    pub fn format_amount(mut self, format: fn(f64) -> String) -> Self {
        self.format = format;
        self
    }
}

impl RenderOnce for ValueBars {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let values: Vec<f64> = self.rows.iter().map(|r| r.value).collect();
        let fractions = value_fractions(&values);
        let (row_height, track_height, name_style, amount_style) = if self.compact {
            (COMPACT_ROW_HEIGHT, COMPACT_TRACK_HEIGHT, LABEL, MONO_SM)
        } else {
            (ROW_HEIGHT, TRACK_HEIGHT, BODY, MONO)
        };
        let radius = track_height / 2.0;
        let format = self.format;

        div().flex().flex_col().w_full().children(self.rows.into_iter().zip(fractions).map(
            move |(row, frac)| {
                let fill = if row.spawn { SPAWN_BAR } else { TEXT_MUTED };
                let amount = row.amount.unwrap_or_else(|| format(row.value).into());
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(COLUMN_GAP))
                    .h(px(row_height))
                    .child(
                        div()
                            .w(px(NAME_WIDTH))
                            .flex_none()
                            .flex()
                            .type_style(name_style)
                            .text_color(TEXT_SECONDARY)
                            .child(truncate_lh(row.name, name_style.line)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(track_height))
                            .rounded(px(radius))
                            .bg(LINE)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(frac))
                                    .rounded(px(radius))
                                    .bg(fill),
                            ),
                    )
                    .child(
                        div()
                            .w(px(AMOUNT_WIDTH))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .type_style(amount_style)
                            .text_color(TEXT)
                            .whitespace_nowrap()
                            .child(amount),
                    )
            },
        ))
    }
}
