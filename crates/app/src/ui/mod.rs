//! Reusable components matching `design/screens/*.html`. Screens compose these instead of
//! styling raw divs, so the whole app stays consistent.
//!
//! | Component | Use |
//! |---|---|
//! | [`Button`], [`IconButton`] | actions (primary / secondary / ghost / danger, md / sm) |
//! | [`Badge`], [`count_pill`], [`kbd`], [`VariantChip`] | small labels |
//! | [`StatusDot`], [`StatusPill`] | task / run status (running pulses) |
//! | [`card`], [`panel`], [`SectionHeader`], [`PageHeader`], [`key_value`], [`divider`] | layout |
//! | [`EmptyState`], [`Skeleton`] | no data / loading |
//! | [`Tabs`] | underline tabs ("Graph Runs Versions") |
//! | [`ProgressBar`], [`MeterRow`] | animated bars |
//! | [`BarChart`], [`Sparkline`] | analytics |
//! | [`DiffView`] / [`ParsedDiff`] | unified diffs |
//! | [`markdown`], [`CodeBlock`] | message text |
//! | [`input`] (`text_input`, `composer`, `field`), [`select`] | form controls |
//! | [`icon`] | an [`AppIcon`] that inherits text color |
//!
//! Everything is a `RenderOnce` or a function returning a styled `Div`, so it can be
//! extended with the usual gpui style methods.

// A component library: not every component or helper is used by every screen yet.
#![allow(dead_code, unused_imports)]

mod badge;
mod button;
mod chart;
mod diff;
mod layout;
mod progress;
mod tabs;
mod text;

pub mod input;
pub mod select;

pub use badge::*;
pub use button::*;
pub use chart::*;
pub use diff::*;
pub use layout::*;
pub use progress::*;
pub use tabs::*;
pub use text::*;

pub use crate::assets::AppIcon;

use gpui::{px, Pixels, Styled};
use gpui_component::Icon;

/// A 16px icon that takes the surrounding text color. Resize with `.size(px(14.))`,
/// recolor with `.text_color(theme::TEXT_MUTED)`.
pub fn icon(name: AppIcon) -> Icon {
    Icon::new(name).size(px(16.))
}

/// An icon of a given size.
pub fn icon_sized(name: AppIcon, size: Pixels) -> Icon {
    Icon::new(name).size(size)
}

/// Short human duration: `45s`, `12m`, `1h 12m`, `3d`.
pub fn fmt_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{}h", secs / 3600)
        } else {
            format!("{}h {}m", secs / 3600, m)
        }
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Time since `t`, short (`6m`, `1h 12m`).
pub fn fmt_ago(t: chrono::DateTime<chrono::Utc>) -> String {
    fmt_duration((chrono::Utc::now() - t).num_seconds())
}

/// `$3.81`; cents below a dollar keep two decimals, large values drop them.
pub fn fmt_usd(v: f64) -> String {
    if v >= 1000.0 {
        format!("${:.0}", v)
    } else {
        format!("${:.2}", v)
    }
}
