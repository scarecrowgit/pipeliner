//! Graphite motion (`~/Pictures/designs/Graphite`, "Motion"): duration and easing tokens,
//! popup presence (enter / exit) and state colour fades.
//!
//! - State changes (background, border, colour on hover / select) fade over
//!   [`DURATION_STATE`] with `ease`.
//! - Popups come out of their trigger and leave faster than they came: popovers 180ms in
//!   with 4px of travel, dialogs 220ms with 8px and a fading scrim, toasts 240ms rising 8px;
//!   every exit is 100ms with at most 2px of travel.
//! - Only opacity and position move (gpui has no transform on divs, so the design's
//!   0.98 → 1 scale is left out). Nothing bounces.
//! - `HARNESS_REDUCE_MOTION=1` (or GNOME's `enable-animations` off) turns motion off:
//!   popups appear and leave at once. `HARNESS_MOTION_SLOW=<n>` stretches every duration
//!   n× (for checking animations by eye).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, App, ElementId, Entity, Hsla, prelude::*,
    Rgba, Stateful, Window, div, px,
};

// ── Tokens ──────────────────────────────────────────────────────────────

/// Any popup leaving: popover, dialog, toast.
pub const DURATION_EXIT: Duration = Duration::from_millis(100);
/// Background, border and colour changes on hover, select and focus.
pub const DURATION_STATE: Duration = Duration::from_millis(150);
/// Popover, menu, picker, and a confirm revealed inside an open popup.
pub const DURATION_ENTER: Duration = Duration::from_millis(180);
/// Dialog and command palette panel, and their scrim.
pub const DURATION_OVERLAY: Duration = Duration::from_millis(220);
/// Toast rising from the bottom edge.
pub const DURATION_TOAST: Duration = Duration::from_millis(240);

/// `cubic-bezier(0.2, 0, 0, 1)`: everything that enters.
pub fn ease_out(t: f32) -> f32 {
    cubic_bezier(0.2, 0., 0., 1., t)
}

/// `cubic-bezier(0.4, 0, 1, 1)`: everything that leaves.
pub fn ease_in(t: f32) -> f32 {
    cubic_bezier(0.4, 0., 1., 1., t)
}

/// CSS `ease` = `cubic-bezier(0.25, 0.1, 0.25, 1)`: state fades.
pub fn ease_standard(t: f32) -> f32 {
    cubic_bezier(0.25, 0.1, 0.25, 1., t)
}

/// CSS cubic-bezier timing function: solve x(s) = t for s, return y(s).
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let bez = |a: f32, b: f32, s: f32| {
        let u = 1. - s;
        3. * u * u * s * a + 3. * u * s * s * b + s * s * s
    };
    let dbez = |a: f32, b: f32, s: f32| {
        let u = 1. - s;
        3. * u * u * a + 6. * u * s * (b - a) + 3. * s * s * (1. - b)
    };
    // Newton, then bisection if the slope is too flat.
    let mut s = t;
    for _ in 0..8 {
        let x = bez(x1, x2, s) - t;
        if x.abs() < 1e-5 {
            return bez(y1, y2, s);
        }
        let d = dbez(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - x / d).clamp(0., 1.);
    }
    let (mut lo, mut hi) = (0f32, 1f32);
    s = t;
    for _ in 0..30 {
        let x = bez(x1, x2, s);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.;
    }
    bez(y1, y2, s)
}

/// Motion is off (`HARNESS_REDUCE_MOTION=1`, or GNOME `enable-animations` is false).
pub fn reduced() -> bool {
    static REDUCED: OnceLock<bool> = OnceLock::new();
    *REDUCED.get_or_init(|| {
        if let Ok(v) = std::env::var("HARNESS_REDUCE_MOTION") {
            return v == "1" || v.eq_ignore_ascii_case("true");
        }
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "false")
    })
}

/// `d` stretched by `HARNESS_MOTION_SLOW` (default 1).
pub fn scaled(d: Duration) -> Duration {
    static SLOW: OnceLock<f32> = OnceLock::new();
    let k = *SLOW.get_or_init(|| {
        std::env::var("HARNESS_MOTION_SLOW")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|k| *k > 0.)
            .unwrap_or(1.)
    });
    d.mul_f32(k)
}

fn progress(since: Instant, d: Duration) -> f32 {
    let d = scaled(d).as_secs_f32();
    if d <= 0. {
        1.
    } else {
        (since.elapsed().as_secs_f32() / d).min(1.)
    }
}

// ── Popup presence ──────────────────────────────────────────────────────

/// What kind of popup, which sets timings and travel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pop {
    /// Popover, menu or picker under its trigger: enters 4px down out of it.
    Below,
    /// Popover above its trigger: enters 4px up out of it.
    Above,
    /// Dialog / command palette panel: 8px up.
    Dialog,
    /// A dialog's scrim: fades only.
    Scrim,
    /// Toast rising 8px from the bottom edge.
    Toast,
}

impl Pop {
    fn enter(self) -> Duration {
        match self {
            Pop::Below | Pop::Above => DURATION_ENTER,
            Pop::Dialog | Pop::Scrim => DURATION_OVERLAY,
            Pop::Toast => DURATION_TOAST,
        }
    }

    /// Offset (px, + = down) at the start of the enter.
    fn enter_dy(self) -> f32 {
        match self {
            Pop::Below => -4.,
            Pop::Above => 4.,
            Pop::Dialog | Pop::Toast => 8.,
            Pop::Scrim => 0.,
        }
    }

    /// Offset at the end of the exit (at most 2px).
    fn exit_dy(self) -> f32 {
        match self {
            Pop::Below => -2.,
            Pop::Above | Pop::Toast => 2.,
            Pop::Dialog | Pop::Scrim => 0.,
        }
    }
}

/// One frame of a popup: how visible it is and how far from its resting place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopFrame {
    pub opacity: f32,
    /// Offset from the resting position in px (+ = down).
    pub dy: f32,
    /// Leaving: the popup should not take clicks.
    pub leaving: bool,
}

impl PopFrame {
    pub const SHOWN: PopFrame = PopFrame {
        opacity: 1.,
        dy: 0.,
        leaving: false,
    };

    /// Frame of `kind` given whether it is open and when that last changed
    /// (`None` = never changed: no animation).
    pub fn at(kind: Pop, open: bool, since: Option<Instant>) -> Option<PopFrame> {
        if reduced() {
            return open.then_some(PopFrame::SHOWN);
        }
        match (open, since) {
            (true, None) => Some(PopFrame::SHOWN),
            (false, None) => None,
            (true, Some(t0)) => {
                let e = ease_out(progress(t0, kind.enter()));
                Some(PopFrame {
                    opacity: e,
                    dy: kind.enter_dy() * (1. - e),
                    leaving: false,
                })
            }
            (false, Some(t0)) => {
                let p = progress(t0, DURATION_EXIT);
                (p < 1.).then(|| {
                    let e = ease_in(p);
                    PopFrame {
                        opacity: 1. - e,
                        dy: kind.exit_dy() * e,
                        leaving: true,
                    }
                })
            }
        }
    }

    pub fn settled(&self) -> bool {
        self.opacity >= 1. && self.dy == 0. && !self.leaving
    }
}

/// Open/closed and when it last changed, kept per popup between frames.
#[derive(Debug, Default)]
pub struct Presence {
    open: bool,
    since: Option<Instant>,
}

impl Presence {
    pub fn set(&mut self, open: bool) {
        if open != self.open {
            self.open = open;
            self.since = Some(Instant::now());
        }
    }

    pub fn frame(&self, kind: Pop) -> Option<PopFrame> {
        PopFrame::at(kind, self.open, self.since)
    }
}

/// Track popup `key` (unique within the view) through `open` changes. Call it every
/// render whether or not the popup is open: it returns the frame to draw, or `None` once
/// the popup is closed and its exit has played. Requests frames while animating.
pub fn presence(
    key: impl Into<ElementId>,
    open: bool,
    kind: Pop,
    window: &mut Window,
    cx: &mut App,
) -> Option<PopFrame> {
    let state: Entity<Presence> = window.use_keyed_state(key, cx, |_, _| Presence::default());
    let frame = state.update(cx, |p, _| {
        p.set(open);
        p.frame(kind)
    });
    if frame.is_some_and(|f| !f.settled()) {
        window.request_animation_frame();
    }
    frame
}

/// Position wrapper of a popover under its trigger, `gap` px away (the wrapper's top
/// padding), moved and faded by frame `f`.
pub fn pop_below(anchor: gpui::Div, gap: f32, f: PopFrame) -> gpui::Div {
    anchor.pt(px((gap + f.dy).max(0.))).opacity(f.opacity)
}

/// Position wrapper of a popover above its trigger (`gap` = bottom padding).
pub fn pop_above(anchor: gpui::Div, gap: f32, f: PopFrame) -> gpui::Div {
    anchor.pb(px((gap - f.dy).max(0.))).opacity(f.opacity)
}

/// Enter-only popup animation for an element that is mounted while open (for popups
/// whose owner does not track presence): fades in and travels like `kind` over its enter
/// duration. Plays each time the element is mounted.
pub fn pop_in<E>(id: impl Into<ElementId>, kind: Pop, el: E) -> gpui::AnyElement
where
    E: Styled + IntoElement + 'static,
{
    if reduced() {
        return el.into_any_element();
    }
    let dy = kind.enter_dy();
    let anim = Animation::new(scaled(kind.enter())).with_easing(ease_out);
    el.with_animation(id, anim, move |el, t| el.opacity(t).mt(px(dy * (1. - t))))
        .into_any_element()
}

/// Appear-only animation for a confirm or banner revealed inside an open popup
/// (`.gr-reveal`: 180ms, 4px down). Plays each time the element is mounted.
pub fn reveal(id: impl Into<ElementId>, el: impl IntoElement) -> gpui::AnyElement {
    let el = div().flex().flex_col().child(el);
    if reduced() {
        return el.into_any_element();
    }
    let anim = Animation::new(scaled(DURATION_ENTER)).with_easing(ease_out);
    el.with_animation(id, anim, |d, t| d.opacity(t).mt(px(-4. * (1. - t))))
        .into_any_element()
}

// ── State fades ─────────────────────────────────────────────────────────

/// A colour easing from `from` to `to` since `since`.
#[derive(Clone, Copy, Debug)]
struct ColorFade {
    from: Hsla,
    to: Hsla,
    since: Instant,
}

impl ColorFade {
    fn value(&self) -> Hsla {
        if reduced() {
            return self.to;
        }
        mix(self.from, self.to, ease_standard(progress(self.since, DURATION_STATE)))
    }

    fn animating(&self) -> bool {
        !reduced() && progress(self.since, DURATION_STATE) < 1.
    }
}

/// Hover flag plus up to three fading colours (background, text, border) of one element.
#[derive(Debug, Default)]
pub struct StateFade {
    pub hovered: bool,
    colors: [Option<ColorFade>; 3],
}

impl StateFade {
    /// Ease slot `slot` toward `target`; returns the colour to draw now.
    fn toward(&mut self, slot: usize, target: Hsla) -> Hsla {
        match &mut self.colors[slot] {
            Some(f) if f.to == target => f.value(),
            Some(f) => {
                let now = f.value();
                *f = ColorFade {
                    from: now,
                    to: target,
                    since: Instant::now(),
                };
                now
            }
            None => {
                // First frame: start at the target, no fade.
                self.colors[slot] = Some(ColorFade {
                    from: target,
                    to: target,
                    since: Instant::now(),
                });
                target
            }
        }
    }

    fn animating(&self) -> bool {
        self.colors.iter().flatten().any(ColorFade::animating)
    }
}

/// Colours of an element at rest or when hovered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub bg: Hsla,
    pub fg: Option<Hsla>,
    pub border: Option<Hsla>,
}

impl Look {
    pub fn bg(bg: Hsla) -> Self {
        Self {
            bg,
            fg: None,
            border: None,
        }
    }

    pub fn fg(mut self, fg: Hsla) -> Self {
        self.fg = Some(fg);
        self
    }

    pub fn border(mut self, border: Hsla) -> Self {
        self.border = Some(border);
        self
    }
}

/// Hover state of element `key`, for looks that depend on it beyond colours.
pub fn hovered(key: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> bool {
    let state: Entity<StateFade> = window.use_keyed_state(key, cx, |_, _| StateFade::default());
    state.read(cx).hovered
}

/// Give `el` the colours of `rest`, or `hover` while the pointer is over it, fading
/// between them (and between any later changes of `rest`, e.g. selection) over 150ms.
/// `key` must be unique within the view; use the element's own id.
pub fn hover_fade(
    el: Stateful<gpui::Div>,
    key: impl Into<ElementId>,
    rest: Look,
    hover: Look,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<gpui::Div> {
    let state: Entity<StateFade> = window.use_keyed_state(key, cx, |_, _| StateFade::default());
    let (bg, fg, border, animating) = state.update(cx, |s, _| {
        let look = if s.hovered { hover } else { rest };
        let bg = s.toward(0, look.bg);
        let fg = look.fg.map(|c| s.toward(1, c));
        let border = look.border.map(|c| s.toward(2, c));
        (bg, fg, border, s.animating())
    });
    if animating {
        window.request_animation_frame();
    }
    let mut el = el.bg(bg);
    if let Some(fg) = fg {
        el = el.text_color(fg);
    }
    if let Some(b) = border {
        el = el.border_color(b);
    }
    el.on_hover(move |h, _, cx| {
        state.update(cx, |s, cx| {
            if s.hovered != *h {
                s.hovered = *h;
                cx.notify();
            }
        })
    })
}

/// [`hover_fade`] as an element, for builders without a `Window` at hand: wrap a finished
/// row (`motion::fade(row, id, rest, hover)`) and the fade is applied when it renders.
#[derive(IntoElement)]
pub struct Fade {
    el: Stateful<gpui::Div>,
    key: ElementId,
    rest: Look,
    hover: Look,
}

pub fn fade(el: Stateful<gpui::Div>, key: impl Into<ElementId>, rest: Look, hover: Look) -> Fade {
    Fade {
        el,
        key: key.into(),
        rest,
        hover,
    }
}

/// Look pair of a menu or list row: `selected` stays on `surface-hover`, others go from
/// transparent to `hover_bg` on hover.
pub fn row_looks(selected: bool, hover_bg: Hsla) -> (Look, Look) {
    if selected {
        let l = Look::bg(crate::theme::SURFACE_HOVER);
        (l, l)
    } else {
        (Look::bg(gpui::transparent_black()), Look::bg(hover_bg))
    }
}

// A `Fade` chains like the row it wraps (children, styles, handlers go to the row).
impl ParentElement for Fade {
    fn extend(&mut self, elements: impl IntoIterator<Item = gpui::AnyElement>) {
        self.el.extend(elements)
    }
}

impl Styled for Fade {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.el.style()
    }
}

impl InteractiveElement for Fade {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.el.interactivity()
    }
}

impl StatefulInteractiveElement for Fade {}

/// A menu / list row (`.gr-menu-item`, table row): `selected` sits on `surface-hover`,
/// others fade to `hover_bg` on hover.
pub fn fade_row(el: Stateful<gpui::Div>, key: impl Into<ElementId>, selected: bool, hover_bg: Hsla) -> Fade {
    let (rest, hover) = row_looks(selected, hover_bg);
    fade(el, key, rest, hover)
}

impl RenderOnce for Fade {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        hover_fade(self.el, self.key, self.rest, self.hover, window, cx)
    }
}

/// Fade one colour (e.g. a tab label or an icon) toward `target` over 150ms.
pub fn fade_color(
    key: impl Into<ElementId>,
    target: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> Hsla {
    let state: Entity<StateFade> = window.use_keyed_state(key, cx, |_, _| StateFade::default());
    let (c, animating) = state.update(cx, |s, _| (s.toward(0, target), s.animating()));
    if animating {
        window.request_animation_frame();
    }
    c
}

/// Mix two colours in RGB; a fully transparent end takes the other end's hue, so a
/// transparent background fades in rather than through black.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0., 1.);
    if t <= 0. {
        return a;
    }
    if t >= 1. {
        return b;
    }
    let (ra, rb): (Rgba, Rgba) = (a.to_rgb(), b.to_rgb());
    let (ra, rb) = match (a.a == 0., b.a == 0.) {
        (true, false) => (Rgba { a: 0., ..rb }, rb),
        (false, true) => (ra, Rgba { a: 0., ..ra }),
        _ => (ra, rb),
    };
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: l(ra.r, rb.r),
        g: l(ra.g, rb.g),
        b: l(ra.b, rb.b),
        a: l(ra.a, rb.a),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_endpoints_and_shape() {
        for f in [ease_out, ease_in, ease_standard] {
            assert!(f(0.).abs() < 1e-3);
            assert!((f(1.) - 1.).abs() < 1e-3);
        }
        // ease-out is ahead of linear at the midpoint, ease-in behind.
        assert!(ease_out(0.5) > 0.7);
        assert!(ease_in(0.5) < 0.4);
        // monotonic
        let mut last = 0.;
        for i in 0..=100 {
            let v = ease_out(i as f32 / 100.);
            assert!(v + 1e-4 >= last);
            last = v;
        }
    }

    #[test]
    fn pop_frames() {
        assert_eq!(PopFrame::at(Pop::Below, false, None), None);
        assert_eq!(PopFrame::at(Pop::Below, true, None), Some(PopFrame::SHOWN));
        if reduced() {
            return;
        }
        let f = PopFrame::at(Pop::Below, true, Some(Instant::now())).unwrap();
        assert!(f.opacity < 0.1 && f.dy < -3.5 && !f.leaving);
        let old = Instant::now() - Duration::from_secs(1);
        assert_eq!(PopFrame::at(Pop::Dialog, true, Some(old)), Some(PopFrame::SHOWN));
        assert_eq!(PopFrame::at(Pop::Dialog, false, Some(old)), None);
        let f = PopFrame::at(Pop::Toast, false, Some(Instant::now())).unwrap();
        assert!(f.leaving && f.opacity > 0.9);
    }

    #[test]
    fn mix_from_transparent_keeps_hue() {
        let a = gpui::transparent_black();
        let b: Hsla = Rgba { r: 1., g: 0., b: 0., a: 1. }.into();
        let m = mix(a, b, 0.5).to_rgb();
        assert!((m.r - 1.).abs() < 1e-3 && m.g.abs() < 1e-3);
        assert!((m.a - 0.5).abs() < 1e-3);
    }
}
