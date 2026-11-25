use std::time::Instant;

use gpui::{
    div, px, relative, App, ElementId, Hsla, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window,
};

use crate::theme::{self, TextStyles as _};

/// Remembers the last value so width changes animate instead of jumping.
struct BarMemory {
    from: f32,
    to: f32,
    start: Instant,
}

impl BarMemory {
    fn current(&self) -> (f32, bool) {
        let dur = theme::DUR_OVERLAY.as_secs_f32() * 1.5;
        let t = (self.start.elapsed().as_secs_f32() / dur).min(1.0);
        (self.from + (self.to - self.from) * theme::ease_out(t), t >= 1.0)
    }
}

/// 4px progress bar whose width animates (on first show and on every change).
///
/// `ProgressBar::new(("spend", ix), spent / budget).color(theme::WARNING)`.
/// The id must be stable across renders for the animation to track the value.
#[derive(IntoElement)]
pub struct ProgressBar {
    id: ElementId,
    value: f32,
    color: Option<Hsla>,
    height: gpui::Pixels,
    auto_tone: bool,
}

impl ProgressBar {
    /// `value` is a fraction, clamped to 0..=1.
    pub fn new(id: impl Into<ElementId>, value: f64) -> Self {
        ProgressBar {
            id: id.into(),
            value: if value.is_finite() { value.clamp(0.0, 1.0) as f32 } else { 0.0 },
            color: None,
            height: px(4.),
            auto_tone: false,
        }
    }
    pub fn color(mut self, c: Hsla) -> Self {
        self.color = Some(c);
        self
    }
    pub fn height(mut self, h: gpui::Pixels) -> Self {
        self.height = h;
        self
    }
    /// Budget bars: accent below 80%, warning from 80%, danger at 100%.
    pub fn budget(mut self) -> Self {
        self.auto_tone = true;
        self
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let target = self.value;
        let mem = window.use_keyed_state(self.id.clone(), cx, |_, _| BarMemory {
            from: 0.0,
            to: target,
            start: Instant::now(),
        });
        let (prev_to, (now, settled)) = {
            let m = mem.read(cx);
            (m.to, m.current())
        };
        let (shown, done) = if (prev_to - target).abs() > f32::EPSILON {
            mem.update(cx, |m, _| {
                m.from = now;
                m.to = target;
                m.start = Instant::now();
            });
            (now, false)
        } else {
            (now, settled)
        };
        if !done {
            window.request_animation_frame();
        }
        let color = self.color.unwrap_or(if self.auto_tone {
            if target >= 1.0 {
                theme::DANGER
            } else if target >= 0.8 {
                theme::WARNING
            } else {
                theme::ACCENT
            }
        } else {
            theme::ACCENT
        });
        div()
            .w_full()
            .h(self.height)
            .flex_none()
            .rounded(theme::RADIUS_FULL)
            .bg(theme::LINE)
            .overflow_hidden()
            .child(div().h_full().w(relative(shown)).rounded(theme::RADIUS_FULL).bg(color))
    }
}

/// Label, horizontal bar, value: one row of a breakdown ("Shipped first time ▬▬▬ 276").
#[derive(IntoElement)]
pub struct MeterRow {
    id: ElementId,
    label: SharedString,
    value: f64,
    value_text: SharedString,
    color: Option<Hsla>,
}

impl MeterRow {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, fraction: f64, value_text: impl Into<SharedString>) -> Self {
        MeterRow { id: id.into(), label: label.into(), value: fraction, value_text: value_text.into(), color: None }
    }
    pub fn color(mut self, c: Hsla) -> Self {
        self.color = Some(c);
        self
    }
}

impl RenderOnce for MeterRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut bar = ProgressBar::new(self.id, self.value).height(px(6.));
        if let Some(c) = self.color {
            bar = bar.color(c);
        }
        div()
            .flex()
            .items_center()
            .gap(theme::S4)
            .h(px(30.))
            .child(div().w(px(150.)).flex_none().text_body().fg_secondary().truncate().child(self.label))
            .child(div().flex_1().child(bar))
            .child(div().w(px(64.)).flex_none().text_mono().fg().text_right().child(self.value_text))
    }
}
