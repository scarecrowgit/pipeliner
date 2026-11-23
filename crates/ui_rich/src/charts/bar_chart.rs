//! The vertical bar chart of Home ("Spend per day", "Spend per week").

use gpui::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Window, div, prelude::*,
    px,
};
use ui::theme::*;
use ui::tooltip::Tooltip;

use super::bar_layout::{Bar, BarGeometry, ChartLayout, layout_bars};
use super::scale::axis_ticks;
use crate::canvas::paint::dashed_rule;

/// Height of the plot, px (`.bars`).
const PLOT_HEIGHT: f32 = 120.0;
/// Width of the y-axis label column.
const AXIS_WIDTH: f32 = 28.0;
/// Space left of the bars: the axis column plus its gap.
const GUTTER: f32 = 32.0;
/// Space between a bar and its value label.
const VALUE_GAP: f32 = 4.0;
/// Space between the plot and the x labels.
const LABELS_GAP: f32 = 6.0;
/// Corner radius of a bar's top (`border-radius: 3px 3px 0 0`).
const BAR_RADIUS: f32 = 3.0;

/// Which bar is the `now` bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Now {
    Last,
    At(usize),
    None,
}

/// Vertical bars against an optional limit.
///
/// 120 px high with a 28 px axis column, a dashed limit line, bars in `line-strong` and the
/// `now` bar in the accent colour, rounded tops, a value label above the `now` bar and the
/// highest bar, x labels below where a bar has one. The part of a bar above the limit is
/// drawn in the danger colour. Hovering a bar shows its tooltip.
///
/// The value at the top of the plot is `max` when set; otherwise the limit when no bar goes
/// past it, otherwise [`nice_ceiling`](super::nice_ceiling) of the largest bar and the limit.
#[derive(IntoElement)]
pub struct BarChart {
    id: ElementId,
    bars: Vec<Bar>,
    limit: Option<f64>,
    max: Option<f64>,
    axis: Option<[SharedString; 3]>,
    now: Now,
    max_labels: Option<usize>,
    format: fn(f64) -> String,
}

impl BarChart {
    /// A chart of `bars`. The id must be unique among the elements of the same parent.
    pub fn new(id: impl Into<ElementId>, bars: impl IntoIterator<Item = Bar>) -> Self {
        Self {
            id: id.into(),
            bars: bars.into_iter().collect(),
            limit: None,
            max: None,
            axis: None,
            now: Now::Last,
            max_labels: None,
            format: base::fmt::cost,
        }
    }

    /// The dashed limit line.
    pub fn limit(mut self, limit: f64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// The value at the top of the plot, instead of the automatic one.
    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    /// The three y-axis labels, top, middle and bottom: `"$10"`, `"$5"`, `"$0"`. Without
    /// them the axis shows whole numbers.
    pub fn axis_labels(
        mut self,
        top: impl Into<SharedString>,
        middle: impl Into<SharedString>,
        bottom: impl Into<SharedString>,
    ) -> Self {
        self.axis = Some([top.into(), middle.into(), bottom.into()]);
        self
    }

    /// The bar drawn in the accent colour (default: the last one).
    pub fn now(mut self, index: usize) -> Self {
        self.now = Now::At(index);
        self
    }

    /// No bar is marked as `now`.
    pub fn without_now(mut self) -> Self {
        self.now = Now::None;
        self
    }

    /// Show at most `n` x labels, evenly spread, whatever the bars carry.
    pub fn max_labels(mut self, n: usize) -> Self {
        self.max_labels = Some(n);
        self
    }

    /// How the value label above a bar is written (default: dollars and cents).
    pub fn format_value(mut self, format: fn(f64) -> String) -> Self {
        self.format = format;
        self
    }
}

fn plain(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v:.1}") }
}

impl RenderOnce for BarChart {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let now = match self.now {
            Now::Last => self.bars.len().checked_sub(1),
            Now::At(i) => Some(i),
            Now::None => None,
        };
        let layout = layout_bars(&self.bars, self.limit, self.max, now, self.max_labels);
        let axis = self.axis.clone().unwrap_or_else(|| axis_ticks(layout.max).map(|v| plain(v).into()));
        let any_label = layout.bars.iter().any(|b| b.show_label);

        let bars = self.bars.iter().zip(&layout.bars).enumerate().map(|(i, (bar, geo))| {
            let value = (geo.show_value).then(|| (self.format)(bar.value));
            column(i, bar, geo, value)
        });

        let plot = div()
            .relative()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(SPACE_2))
            .h(px(PLOT_HEIGHT))
            .pl(px(GUTTER))
            .child(y_axis(axis))
            .when_some(limit_line(&layout), |d, line| d.child(line))
            .children(bars);

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .w_full()
            .child(plot)
            .when(any_label, |d| d.child(x_labels(&self.bars, &layout)))
    }
}

/// The y-axis labels, top to bottom, in the column left of the bars.
fn y_axis(labels: [SharedString; 3]) -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(px(AXIS_WIDTH))
        .flex()
        .flex_col()
        .items_end()
        .justify_between()
        .type_style(MONO_SM)
        .line_height(px(12.))
        .text_color(TEXT_MUTED)
        .children(labels.map(|l| div().child(l)))
}

/// The dashed limit line across the plot, at its height.
fn limit_line(layout: &ChartLayout) -> Option<gpui::Div> {
    let frac = layout.limit?;
    let top = (1.0 - frac) * PLOT_HEIGHT;
    Some(
        div()
            .absolute()
            .left(px(GUTTER))
            .right_0()
            .top(px(top))
            .child(dashed_rule(LINE_STRONG, 3.0, 3.0, 1.0)),
    )
}

/// One bar with its hover tooltip and, for some, the value label above it.
fn column(i: usize, bar: &Bar, geo: &BarGeometry, value: Option<String>) -> impl IntoElement {
    let height = geo.height * PLOT_HEIGHT;
    let over = geo.over * PLOT_HEIGHT;
    let fill = if geo.now { ACCENT } else { LINE_STRONG };
    let tooltip = bar.tooltip.clone();
    div()
        .id(("bar", i))
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .justify_end()
        .when(!tooltip.is_empty(), |d| d.tooltip(Tooltip::text(tooltip)))
        .when_some(value, |d, text| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(height + VALUE_GAP))
                    .flex()
                    .justify_center()
                    .type_style(MONO_SM)
                    .text_color(TEXT_MUTED)
                    .whitespace_nowrap()
                    .child(text),
            )
        })
        .when(over > 0.0, |d| {
            d.child(div().flex_none().h(px(over)).rounded_t(px(BAR_RADIUS)).bg(DANGER))
        })
        .child(
            div()
                .flex_none()
                .h(px(height - over))
                .when(over <= 0.0, |d| d.rounded_t(px(BAR_RADIUS)))
                .bg(fill),
        )
}

/// The x labels, one cell per bar so they line up with the bars.
fn x_labels(bars: &[Bar], layout: &ChartLayout) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .gap(px(SPACE_2))
        .pl(px(GUTTER))
        .mt(px(LABELS_GAP))
        .type_style(MONO_SM)
        .text_color(TEXT_MUTED)
        .children(bars.iter().zip(&layout.bars).map(|(bar, geo)| {
            div()
                .flex_1()
                .min_w_0()
                .h(px(MONO_SM.line))
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .when(geo.show_label, |d| d.child(bar.label.clone()))
        }))
}
