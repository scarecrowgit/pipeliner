//! The period switch: its options, their words and the choice kept while the app runs.

use gpui::{App, Global};
use hub::api::Period;

/// The options of the switch, left to right.
pub(crate) const PERIODS: [Period; 4] = [Period::Today, Period::Days7, Period::Days30, Period::Days90];

/// The switch's label.
pub(crate) fn label(period: Period) -> &'static str {
    match period {
        Period::Today => "Today",
        Period::Days7 => "7 days",
        Period::Days30 => "30 days",
        Period::Days90 => "90 days",
    }
}

/// The name a probe and a script use: `home-period-<name>`.
pub(crate) fn name(period: Period) -> &'static str {
    match period {
        Period::Today => "today",
        Period::Days7 => "7d",
        Period::Days30 => "30d",
        Period::Days90 => "90d",
    }
}

/// The period as a phrase after a heading ("Spend by agent", "last 90 days").
pub(crate) fn span(period: Period) -> &'static str {
    match period {
        Period::Today => "today",
        Period::Days7 => "last 7 days",
        Period::Days30 => "last 30 days",
        Period::Days90 => "last 90 days",
    }
}

/// The spend chart draws weeks for 90 days and days for anything shorter.
pub(crate) fn weekly(period: Period) -> bool {
    period == Period::Days90
}

/// The period the person chose last, kept for the life of the app.
struct Remembered(Period);

impl Global for Remembered {}

/// The period to open Home on.
pub(crate) fn remembered(cx: &App) -> Period {
    cx.try_global::<Remembered>().map_or(Period::Today, |remembered| remembered.0)
}

/// Remembers the choice for the next time Home opens.
pub(crate) fn remember(period: Period, cx: &mut App) {
    cx.set_global(Remembered(period));
}
