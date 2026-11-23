//! Graphite motion (`GRAPHITE.md`, "Motion"): easing, reduced motion, popup presence and
//! state colour fades.
//!
//! - State changes (background, border, colour on hover or select) fade over
//!   [`DURATION_STATE`] with `ease`: see [`hover_fade`], [`fade_row`], [`fade_color`].
//! - Popups come out of their trigger and leave faster than they came: popovers 180 ms in
//!   with 4 px of travel, dialogs 220 ms with 8 px and a fading scrim, toasts 240 ms rising
//!   8 px; every exit is 100 ms with at most 2 px of travel: see [`presence`], [`pop_in`].
//! - Only opacity and position move (GPUI has no transform on divs, so the design's
//!   0.98 to 1 scale is left out). Nothing bounces.
//! - Reduced motion (`HARNESS_REDUCE_MOTION=1`, or GNOME's `enable-animations` off, read once
//!   by [`init`]) makes popups appear and leave at once and state changes apply instantly.
//!   `HARNESS_MOTION_SLOW=<n>` stretches every duration n times, for checking by eye.

mod fade;
mod presence;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use gpui::{Hsla, Rgba};

pub use crate::theme::{
    DURATION_ENTER, DURATION_EXIT, DURATION_OVERLAY, DURATION_STATE, DURATION_TOAST,
};
use crate::theme::{EASE_IN, EASE_OUT, EASE_STANDARD};
pub use fade::{
    Fade, HoverFade, Look, StateFade, fade, fade_color, fade_row, fade_value, hover_fade,
    hover_fade_state, hovered, row_looks,
};
pub use presence::{Pop, PopFrame, Presence, pop_above, pop_below, pop_in, presence, reveal};

/// `cubic-bezier(0.2, 0, 0, 1)`: everything that enters.
pub fn ease_out(t: f32) -> f32 {
    EASE_OUT.at(t)
}

/// `cubic-bezier(0.4, 0, 1, 1)`: everything that leaves.
pub fn ease_in(t: f32) -> f32 {
    EASE_IN.at(t)
}

/// CSS `ease`: state fades.
pub fn ease_standard(t: f32) -> f32 {
    EASE_STANDARD.at(t)
}

// 0 = not resolved yet, 1 = motion on, 2 = motion off.
static REDUCED: AtomicU8 = AtomicU8::new(0);

fn env_reduced() -> Option<bool> {
    std::env::var("HARNESS_REDUCE_MOTION").ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn system_reduced() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "false")
}

/// Read the reduced-motion preference once, at startup (environment, then GNOME settings).
/// [`crate::init`] calls it; until then [`reduced`] only looks at the environment, so
/// rendering never spawns a process.
pub fn init() {
    let off = env_reduced().unwrap_or_else(system_reduced);
    set_reduced(off);
}

/// Turn motion off (`true`) or on, overriding what [`init`] found.
pub fn set_reduced(off: bool) {
    REDUCED.store(if off { 2 } else { 1 }, Ordering::Relaxed);
}

/// Motion is off: popups appear and leave at once, state changes do not fade.
pub fn reduced() -> bool {
    match REDUCED.load(Ordering::Relaxed) {
        0 => {
            let off = env_reduced().unwrap_or(false);
            set_reduced(off);
            off
        }
        v => v == 2,
    }
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

/// How far (0 to 1) an animation of length `d` that started at `since` has run.
pub(crate) fn progress(since: Instant, d: Duration) -> f32 {
    let d = scaled(d).as_secs_f32();
    if d <= 0. { 1. } else { (since.elapsed().as_secs_f32() / d).min(1.) }
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
    Rgba { r: l(ra.r, rb.r), g: l(ra.g, rb.g), b: l(ra.b, rb.b), a: l(ra.a, rb.a) }.into()
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
        let mut last = 0.;
        for i in 0..=100 {
            let v = ease_out(i as f32 / 100.);
            assert!(v + 1e-4 >= last);
            last = v;
        }
    }

    #[test]
    fn mix_from_transparent_keeps_hue() {
        let a = gpui::transparent_black();
        let b: Hsla = Rgba { r: 1., g: 0., b: 0., a: 1. }.into();
        let m = mix(a, b, 0.5).to_rgb();
        assert!((m.r - 1.).abs() < 1e-3 && m.g.abs() < 1e-3);
        assert!((m.a - 0.5).abs() < 1e-3);
    }

    #[test]
    fn mix_ends_are_exact() {
        let a = crate::theme::SURFACE;
        let b = crate::theme::ACCENT;
        assert_eq!(mix(a, b, 0.), a);
        assert_eq!(mix(a, b, 1.), b);
    }
}
