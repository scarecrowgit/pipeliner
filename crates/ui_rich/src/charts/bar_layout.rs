//! The pure layout of a [`BarChart`](super::BarChart): heights, over-limit parts and which
//! labels show. No window needed, so it is tested directly.

use gpui::SharedString;

use super::scale::{fraction, nice_ceiling};

/// One bar: its x label (may be empty), its value and the text of its hover tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    /// Label under the bar; empty for a bar that is not labelled.
    pub label: SharedString,
    /// The value; negative and non-finite values draw as zero.
    pub value: f64,
    /// Shown on hover; empty for none.
    pub tooltip: SharedString,
}

impl Bar {
    /// A bar with an x label and a value, no tooltip.
    pub fn new(label: impl Into<SharedString>, value: f64) -> Self {
        Self { label: label.into(), value, tooltip: SharedString::default() }
    }

    /// The text shown on hover.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = text.into();
        self
    }
}

/// How one bar is drawn, as fractions of the plot height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarGeometry {
    /// Total height, `0..=1`.
    pub height: f32,
    /// The part of `height` above the limit line (drawn in the danger colour); `0` when the
    /// bar stays under the limit or there is none.
    pub over: f32,
    /// A value label sits above this bar (the `now` bar and the highest bar).
    pub show_value: bool,
    /// The x label under this bar shows.
    pub show_label: bool,
    /// This is the `now` bar (accent colour).
    pub now: bool,
}

/// The result of [`layout_bars`].
#[derive(Clone, Debug, PartialEq)]
pub struct ChartLayout {
    /// The value at the top of the plot.
    pub max: f64,
    /// Height of the limit line as a fraction of the plot (`0..=1`), if there is a limit.
    pub limit: Option<f32>,
    /// One entry per input bar.
    pub bars: Vec<BarGeometry>,
}

fn clean(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 { value } else { 0.0 }
}

/// The value at the top of the plot: `max` when given, else the limit when no bar passes it,
/// else a nice ceiling above the largest bar and the limit. Never zero or NaN (an all-zero
/// chart gets `1`).
fn resolve_max(values: &[f64], limit: Option<f64>, max: Option<f64>) -> f64 {
    if let Some(m) = max.filter(|m| m.is_finite() && *m > 0.0) {
        return m;
    }
    let top = values.iter().copied().fold(0.0, f64::max);
    let resolved = match limit {
        Some(l) if top <= l => l,
        Some(l) => nice_ceiling(top.max(l)),
        None => nice_ceiling(top),
    };
    if resolved > 0.0 { resolved } else { 1.0 }
}

/// Which of the labelled positions stay when at most `max_labels` fit: evenly spread, always
/// the first and the last. `labelled[i]` says whether bar `i` has a label; the result has the
/// same length.
///
/// ```text
/// thin_labels(&[true; 13], 5) shows bars 0, 3, 6, 9 and 12
/// ```
pub fn thin_labels(labelled: &[bool], max_labels: usize) -> Vec<bool> {
    let idx: Vec<usize> = (0..labelled.len()).filter(|i| labelled[*i]).collect();
    let mut keep = vec![false; labelled.len()];
    let n = idx.len();
    if n <= max_labels {
        for i in idx {
            keep[i] = true;
        }
        return keep;
    }
    match max_labels {
        0 => {}
        1 => keep[idx[n - 1]] = true,
        m => {
            for j in 0..m {
                let pick = (j as f64 * (n - 1) as f64 / (m - 1) as f64).round() as usize;
                keep[idx[pick.min(n - 1)]] = true;
            }
        }
    }
    keep
}

/// Lays out `bars` against `limit`.
///
/// - `max`: the value at the top of the plot; `None` picks it (see [`BarChart`](super::BarChart)).
/// - `now`: index of the bar drawn in the accent colour; `None` for none.
/// - `max_labels`: thin the x labels to at most this many; `None` shows every non-empty one.
///
/// A value label shows above the `now` bar and above the first highest bar, only for values
/// above zero. Empty input gives an empty layout.
pub fn layout_bars(
    bars: &[Bar],
    limit: Option<f64>,
    max: Option<f64>,
    now: Option<usize>,
    max_labels: Option<usize>,
) -> ChartLayout {
    let values: Vec<f64> = bars.iter().map(|b| clean(b.value)).collect();
    let limit = limit.filter(|l| l.is_finite() && *l > 0.0);
    let top = resolve_max(&values, limit, max);
    let limit_frac = limit.map(|l| fraction(l, top));

    let highest = values
        .iter()
        .enumerate()
        .fold(None::<(usize, f64)>, |best, (i, v)| match best {
            Some((_, b)) if *v <= b => best,
            _ if *v > 0.0 => Some((i, *v)),
            _ => best,
        })
        .map(|(i, _)| i);

    let labelled: Vec<bool> = bars.iter().map(|b| !b.label.trim().is_empty()).collect();
    let show_label = match max_labels {
        Some(m) => thin_labels(&labelled, m),
        None => labelled,
    };

    let geometry = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let height = fraction(*v, top);
            let over = match limit_frac {
                Some(l) if height > l => height - l,
                _ => 0.0,
            };
            let is_now = now == Some(i);
            BarGeometry {
                height,
                over,
                show_value: *v > 0.0 && (is_now || highest == Some(i)),
                show_label: show_label[i],
                now: is_now,
            }
        })
        .collect();
    ChartLayout { max: top, limit: limit_frac, bars: geometry }
}
