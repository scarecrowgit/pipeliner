//! The Gantt timeline of a test run: a label column, a track per row and a time axis.

use gpui::{
    App, IntoElement, ParentElement, RenderOnce, SharedString, Window, div, prelude::*, px,
    relative,
};
use ui::layout::truncate_lh;
use ui::theme::*;

/// Width of the label and meta column (`.tl`).
const LABEL_WIDTH: f32 = 200.0;
/// Space between the label column and the track.
const COLUMN_GAP: f32 = 12.0;
/// Row height, and the bar inside it.
const ROW_HEIGHT: f32 = 22.0;
const BAR_TOP: f32 = 6.0;
const BAR_HEIGHT: f32 = 10.0;
const BAR_RADIUS: f32 = 3.0;
/// A bar is never thinner than this, so an instant still shows.
const BAR_MIN_WIDTH: f32 = 2.0;
/// Height of the axis row and of its labels.
const AXIS_ROW_HEIGHT: f32 = 20.0;
const AXIS_LABEL_HEIGHT: f32 = 16.0;
/// Width of the box a middle tick label is centred in.
const TICK_LABEL_WIDTH: f32 = 48.0;
/// Space between the axis row and the rows.
const AXIS_GAP: f32 = 4.0;
/// The most ticks [`time_ticks`] returns.
const MAX_TICKS: usize = 64;

/// What a bar means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BarTone {
    /// An agent working: accent.
    #[default]
    Agent,
    /// An agent created by another agent: the spawn bar colour, and the label in violet.
    Spawn,
    /// Waiting for a human: warning.
    Waiting,
}

impl BarTone {
    fn color(self) -> gpui::Hsla {
        match self {
            BarTone::Agent => ACCENT,
            BarTone::Spawn => SPAWN_BAR,
            BarTone::Waiting => WARNING,
        }
    }
}

/// One row: a label, a meta text and one bar between two fractions of the run.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineRow {
    label: SharedString,
    meta: SharedString,
    start: f32,
    end: f32,
    tone: BarTone,
}

impl TimelineRow {
    /// A row whose bar runs from `start` to `end`, both fractions of the whole (`0..=1`).
    /// Out-of-range and non-finite values are clamped, and swapped when `end < start`.
    pub fn new(
        label: impl Into<SharedString>,
        meta: impl Into<SharedString>,
        start: f32,
        end: f32,
        tone: BarTone,
    ) -> Self {
        let (start, end) = (unit(start), unit(end));
        Self {
            label: label.into(),
            meta: meta.into(),
            start: start.min(end),
            end: start.max(end),
            tone,
        }
    }

    /// A row whose bar runs from `start_ms` to `end_ms` of a run that took `total_ms`.
    pub fn from_ms(
        label: impl Into<SharedString>,
        meta: impl Into<SharedString>,
        start_ms: u64,
        end_ms: u64,
        total_ms: u64,
        tone: BarTone,
    ) -> Self {
        let (start, end) = fractions(start_ms, end_ms, total_ms);
        Self::new(label, meta, start, end, tone)
    }

    /// The bar as `(start, end)` fractions.
    pub fn span(&self) -> (f32, f32) {
        (self.start, self.end)
    }
}

/// A label on the time axis at a fraction of its width.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    /// Position, `0..=1`.
    pub at: f32,
    /// The text, e.g. `"0:30"`.
    pub label: SharedString,
}

impl Tick {
    /// A tick at `at` (clamped to `0..=1`).
    pub fn new(at: f32, label: impl Into<SharedString>) -> Self {
        Self { at: unit(at), label: label.into() }
    }

    /// `labels` spaced evenly from the first edge to the last.
    pub fn evenly(labels: impl IntoIterator<Item = impl Into<SharedString>>) -> Vec<Tick> {
        let labels: Vec<SharedString> = labels.into_iter().map(Into::into).collect();
        let last = labels.len().saturating_sub(1).max(1) as f32;
        labels.into_iter().enumerate().map(|(i, l)| Tick::new(i as f32 / last, l)).collect()
    }
}

fn unit(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// `start_ms` and `end_ms` as fractions of `total_ms`, clamped to `0..=1` and ordered.
/// A zero `total_ms` gives `(0, 0)`.
pub fn fractions(start_ms: u64, end_ms: u64, total_ms: u64) -> (f32, f32) {
    if total_ms == 0 {
        return (0.0, 0.0);
    }
    let at = |ms: u64| (ms as f64 / total_ms as f64).clamp(0.0, 1.0) as f32;
    let (a, b) = (at(start_ms), at(end_ms));
    (a.min(b), a.max(b))
}

/// Where tick labels go on a run of `total_ms`: every `step_ms` from the start, and the end.
///
/// Returns `(fraction, ms)` pairs. The last full step is dropped when it would sit within a
/// quarter step of the end, so two labels never overlap. With a zero step only the start and
/// the end are returned; a zero total gives just `(0, 0)`. At most 64 ticks.
pub fn time_ticks(total_ms: u64, step_ms: u64) -> Vec<(f32, u64)> {
    if total_ms == 0 {
        return vec![(0.0, 0)];
    }
    let frac = |ms: u64| (ms as f64 / total_ms as f64).clamp(0.0, 1.0) as f32;
    let mut out = vec![(0.0, 0)];
    if step_ms > 0 {
        let mut ms = step_ms;
        while ms < total_ms && out.len() < MAX_TICKS - 1 {
            out.push((frac(ms), ms));
            ms += step_ms;
        }
        if let Some(&(_, last)) = out.last()
            && last > 0
            && total_ms - last < step_ms / 4
        {
            out.pop();
        }
    }
    out.push((1.0, total_ms));
    out
}

/// The Gantt of a test run.
///
/// Rows have a 200 px label column (label left, meta right) and a track with one bar placed
/// by its start and end fractions. The time axis sits above the rows, as in the mockup, or
/// below with [`axis_below`](Self::axis_below); its first label is left aligned at the start,
/// its last right aligned at the end and the others centred on their tick.
#[derive(IntoElement)]
pub struct Timeline {
    rows: Vec<TimelineRow>,
    ticks: Vec<Tick>,
    axis_below: bool,
}

impl Timeline {
    /// A timeline of `rows` without an axis.
    pub fn new(rows: impl IntoIterator<Item = TimelineRow>) -> Self {
        Self { rows: rows.into_iter().collect(), ticks: Vec::new(), axis_below: false }
    }

    /// The labels of the time axis.
    pub fn ticks(mut self, ticks: impl IntoIterator<Item = Tick>) -> Self {
        self.ticks = ticks.into_iter().collect();
        self
    }

    /// Put the axis under the rows instead of above them.
    pub fn axis_below(mut self) -> Self {
        self.axis_below = true;
        self
    }
}

fn row(r: TimelineRow) -> impl IntoElement {
    let spawn = r.tone == BarTone::Spawn;
    let width = (r.end - r.start).max(0.0);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(COLUMN_GAP))
        .h(px(ROW_HEIGHT))
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(SPACE_2))
                .type_style(LABEL)
                .text_color(if spawn { SPAWN_TEXT } else { TEXT })
                .child(truncate_lh(r.label, LABEL.line))
                .child(
                    div()
                        .flex_none()
                        .type_style(MONO_SM)
                        .text_color(TEXT_MUTED)
                        .whitespace_nowrap()
                        .child(r.meta),
                ),
        )
        .child(
            div().relative().flex_1().min_w_0().h_full().child(
                div()
                    .absolute()
                    .top(px(BAR_TOP))
                    .h(px(BAR_HEIGHT))
                    .left(relative(r.start))
                    .w(relative(width))
                    .min_w(px(BAR_MIN_WIDTH))
                    .rounded(px(BAR_RADIUS))
                    .bg(r.tone.color()),
            ),
        )
}

fn axis(ticks: Vec<Tick>) -> impl IntoElement {
    let last = ticks.len().saturating_sub(1);
    let labels = ticks.into_iter().enumerate().map(move |(i, t)| {
        let cell = div().absolute().top_0().whitespace_nowrap();
        if i == 0 {
            cell.left(relative(t.at)).child(t.label)
        } else if i == last {
            cell.right(relative(1.0 - t.at)).child(t.label)
        } else {
            cell.left(relative(t.at))
                .ml(px(-TICK_LABEL_WIDTH / 2.0))
                .w(px(TICK_LABEL_WIDTH))
                .flex()
                .justify_center()
                .child(t.label)
        }
    });
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(COLUMN_GAP))
        .h(px(AXIS_ROW_HEIGHT))
        .child(div().w(px(LABEL_WIDTH)).flex_none())
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(AXIS_LABEL_HEIGHT))
                .type_style(MONO_SM)
                .text_color(TEXT_MUTED)
                .children(labels),
        )
}

impl RenderOnce for Timeline {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let has_axis = !self.ticks.is_empty();
        let below = self.axis_below;
        let axis_el = has_axis.then(|| axis(self.ticks));
        let (above, under) = if below { (None, axis_el) } else { (axis_el, None) };
        div()
            .flex()
            .flex_col()
            .w_full()
            .when_some(above, |d, a| d.child(div().mb(px(AXIS_GAP)).child(a)))
            .children(self.rows.into_iter().map(row))
            .when_some(under, |d, a| d.child(div().mt(px(AXIS_GAP)).child(a)))
    }
}
