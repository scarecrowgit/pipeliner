//! Bar charts with a limit line, horizontal value bars and the timeline (Gantt) rows.
//!
//! These are generic drawing pieces: they take plain numbers and strings and know nothing of
//! runs, agents or money beyond the formatting helpers of `base`.
//!
//! - [`BarChart`]: vertical bars against an optional limit (Home's "Spend per day").
//! - [`ValueBars`]: one row per name with a bar relative to the largest value ("Spend by
//!   agent"), full size or compact for a popover.
//! - [`Timeline`]: the Gantt of a test run, one bar per row on a shared time axis.
//! - [`nice_ceiling`], [`ticks`], [`axis_ticks`]: the scale helpers, pure and tested.
//!
//! Layout decisions (bar heights, which labels show, bar fractions) are pure functions
//! ([`layout_bars`], [`value_fractions`], [`fractions`], [`time_ticks`]) so they are tested
//! without a window.

mod bar_chart;
mod bar_layout;
mod scale;
mod timeline;
mod value_bars;

#[cfg(test)]
mod tests;

pub use bar_chart::BarChart;
pub use bar_layout::{Bar, BarGeometry, ChartLayout, layout_bars, thin_labels};
pub use scale::{axis_ticks, fraction, nice_ceiling, ticks};
pub use timeline::{BarTone, Tick, Timeline, TimelineRow, fractions, time_ticks};
pub use value_bars::{ValueBars, ValueRow, value_fractions};
