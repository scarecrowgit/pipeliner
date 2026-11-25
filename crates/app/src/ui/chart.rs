//! Small, dependency-free charts drawn with divs (bars) and a canvas path (sparkline).
//! Feed them real numbers only; with no data show an [`super::EmptyState`] instead.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    canvas, div, point, px, relative, Animation, AnimationExt as _, App, ElementId, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::tooltip::Tooltip;

use crate::theme::{self, TextStyles as _};

/// Rounds up to a "nice" axis maximum (1, 2, 2.5, 5 x 10^k).
pub fn nice_max(v: f64) -> f64 {
    if v.is_nan() || v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let exp = v.log10().floor();
    let base = 10f64.powf(exp);
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if v <= m * base + 1e-9 {
            return m * base;
        }
    }
    10.0 * base
}

/// Vertical bar chart ("Spend per day"). Bars grow in on first show.
///
/// ```ignore
/// BarChart::new("spend-per-day", per_day)
///     .highlight(over_limit)            // bars drawn in warning color
///     .tooltips(day_labels)             // hover text per bar
///     .limit(Some(6.0), "limit $6")     // dashed line
///     .x_labels("Oct 27", "Nov 10", "today")
///     .format(|v| format!("${v:.0}"))
/// ```
#[derive(IntoElement)]
pub struct BarChart {
    id: SharedString,
    values: Vec<f64>,
    highlight: Vec<bool>,
    tooltips: Vec<SharedString>,
    limit: Option<(f64, SharedString)>,
    x_labels: Option<[SharedString; 3]>,
    height: Pixels,
    color: Hsla,
    highlight_color: Hsla,
    format: Rc<dyn Fn(f64) -> String>,
}

impl BarChart {
    pub fn new(id: impl Into<SharedString>, values: Vec<f64>) -> Self {
        BarChart {
            id: id.into(),
            values,
            highlight: vec![],
            tooltips: vec![],
            limit: None,
            x_labels: None,
            height: px(160.),
            color: theme::ACCENT,
            highlight_color: theme::WARNING,
            format: Rc::new(|v| format!("{v:.0}")),
        }
    }
    pub fn highlight(mut self, h: Vec<bool>) -> Self {
        self.highlight = h;
        self
    }
    pub fn tooltips(mut self, t: Vec<SharedString>) -> Self {
        self.tooltips = t;
        self
    }
    pub fn limit(mut self, limit: Option<f64>, label: impl Into<SharedString>) -> Self {
        self.limit = limit.filter(|l| *l > 0.0).map(|l| (l, label.into()));
        self
    }
    pub fn x_labels(
        mut self,
        start: impl Into<SharedString>,
        mid: impl Into<SharedString>,
        end: impl Into<SharedString>,
    ) -> Self {
        self.x_labels = Some([start.into(), mid.into(), end.into()]);
        self
    }
    pub fn height(mut self, h: Pixels) -> Self {
        self.height = h;
        self
    }
    pub fn color(mut self, c: Hsla) -> Self {
        self.color = c;
        self
    }
    /// Y-axis label format.
    pub fn format(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
        self.format = Rc::new(f);
        self
    }
}

impl RenderOnce for BarChart {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let data_max = self.values.iter().cloned().fold(0.0f64, f64::max);
        let max = nice_max(data_max.max(self.limit.as_ref().map(|l| l.0 * 1.2).unwrap_or(0.0)));
        let fmt = self.format.clone();
        let axis_label = |v: f64| div().text_mono_sm().fg_muted().child(fmt(v));
        let id = self.id.clone();
        let n = self.values.len();

        let plot = div()
            .relative()
            .flex_1()
            .h(self.height)
            // grid
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(theme::LINE_SUBTLE),
            )
            .child(
                div()
                    .absolute()
                    .top(relative(0.5))
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(theme::LINE_SUBTLE),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(theme::LINE),
            )
            // bars
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .items_end()
                    .gap(if n > 40 { px(2.) } else { px(4.) })
                    .children(self.values.iter().enumerate().map(|(i, v)| {
                        let frac = (*v / max).clamp(0.0, 1.0) as f32;
                        let color = if self.highlight.get(i).copied().unwrap_or(false) {
                            self.highlight_color
                        } else {
                            self.color
                        };
                        let tip = self.tooltips.get(i).cloned();
                        div()
                            .id(ElementId::NamedInteger(id.clone(), i as u64))
                            .flex_1()
                            .min_w(px(2.))
                            .rounded_t(px(2.))
                            .bg(color)
                            .hover(|s| s.opacity(0.8))
                            .when_some(tip, |d, t| {
                                d.tooltip(move |window, cx| {
                                    Tooltip::new(t.clone()).build(window, cx)
                                })
                            })
                            .with_animation(
                                ElementId::NamedInteger(format!("{id}-grow").into(), i as u64),
                                Animation::new(theme::DUR_OVERLAY * 2).with_easing(theme::ease_out),
                                move |d, t| d.h(relative(frac * t)),
                            )
                    })),
            )
            // limit line
            .when_some(self.limit.clone(), |d, (limit, label)| {
                let top = (1.0 - (limit / max).clamp(0.0, 1.0)) as f32;
                d.child(
                    div()
                        .absolute()
                        .top(relative(top))
                        .left_0()
                        .right_0()
                        .border_t_1()
                        .border_dashed()
                        .border_color(theme::TEXT_MUTED),
                )
                .child(
                    div()
                        .absolute()
                        .top(relative(top))
                        .right_0()
                        .mt(px(-18.))
                        .text_mono_sm()
                        .fg_secondary()
                        .child(label),
                )
            });

        div()
            .flex()
            .flex_col()
            .gap(theme::S2)
            .w_full()
            .child(
                div()
                    .flex()
                    .gap(theme::S3)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_between()
                            .items_end()
                            .w(px(36.))
                            .h(self.height)
                            .mt(px(-8.))
                            .mb(px(-8.))
                            .child(axis_label(max))
                            .child(axis_label(max / 2.0))
                            .child(axis_label(0.0)),
                    )
                    .child(plot),
            )
            .when_some(self.x_labels, |d, [a, b, c]| {
                d.child(
                    div()
                        .flex()
                        .justify_between()
                        .pl(px(48.))
                        .text_mono_sm()
                        .fg_muted()
                        .child(a)
                        .child(b)
                        .child(c),
                )
            })
    }
}

/// A thin line over a series ("Your median time to decide"). Fades in.
#[derive(IntoElement)]
pub struct Sparkline {
    id: ElementId,
    values: Vec<f64>,
    color: Hsla,
    height: Pixels,
    stroke: Pixels,
    zero_based: bool,
}

impl Sparkline {
    pub fn new(id: impl Into<ElementId>, values: Vec<f64>) -> Self {
        Sparkline {
            id: id.into(),
            values,
            color: theme::ACCENT,
            height: px(120.),
            stroke: px(1.5),
            zero_based: true,
        }
    }
    pub fn color(mut self, c: Hsla) -> Self {
        self.color = c;
        self
    }
    pub fn height(mut self, h: Pixels) -> Self {
        self.height = h;
        self
    }
    /// Scale from the data minimum instead of zero.
    pub fn fit(mut self) -> Self {
        self.zero_based = false;
        self
    }
}

impl RenderOnce for Sparkline {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let values = self.values;
        let color = self.color;
        let stroke = self.stroke;
        let zero = self.zero_based;
        div()
            .w_full()
            .h(self.height)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        if values.len() < 2 {
                            return;
                        }
                        let max = values.iter().cloned().fold(f64::MIN, f64::max);
                        let min = if zero {
                            0.0
                        } else {
                            values.iter().cloned().fold(f64::MAX, f64::min)
                        };
                        let span = if (max - min).abs() < 1e-12 {
                            1.0
                        } else {
                            max - min
                        };
                        let inset = stroke;
                        let h = bounds.size.height - inset * 2.;
                        let w = bounds.size.width;
                        let last = (values.len() - 1) as f32;
                        let mut path = PathBuilder::stroke(stroke);
                        for (i, v) in values.iter().enumerate() {
                            let x = bounds.origin.x + w * (i as f32 / last);
                            let y = bounds.origin.y + inset + h * (1.0 - ((v - min) / span) as f32);
                            if i == 0 {
                                path.move_to(point(x, y));
                            } else {
                                path.line_to(point(x, y));
                            }
                        }
                        if let Ok(p) = path.build() {
                            window.paint_path(p, color);
                        }
                    },
                )
                .size_full(),
            )
            .with_animation(
                self.id,
                Animation::new(theme::DUR_OVERLAY * 2).with_easing(theme::ease_out),
                |d, t| d.opacity(t),
            )
    }
}
