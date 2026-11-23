//! Popup presence: enter and exit frames for popovers, dialogs, scrims and toasts.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, App, Div, ElementId, Entity, IntoElement, Styled, Window, div,
    prelude::*, px,
};

use super::{
    DURATION_ENTER, DURATION_EXIT, DURATION_OVERLAY, DURATION_TOAST, ease_in, ease_out, progress,
    reduced, scaled,
};

/// What kind of popup, which sets timings and travel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pop {
    /// Popover, menu or picker under its trigger: enters 4px down out of it.
    Below,
    /// Popover above its trigger: enters 4px up out of it.
    Above,
    /// Dialog or command palette panel: 8px up.
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

    /// Offset (px, positive is down) at the start of the enter.
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
    /// 0 to 1.
    pub opacity: f32,
    /// Offset from the resting position in px (positive is down).
    pub dy: f32,
    /// Leaving: the popup should not take clicks.
    pub leaving: bool,
}

impl PopFrame {
    /// Fully shown and at rest.
    pub const SHOWN: PopFrame = PopFrame { opacity: 1., dy: 0., leaving: false };

    /// Frame of `kind` given whether it is open and when that last changed (`None` is never
    /// changed: no animation). `None` back means the popup is gone.
    pub fn at(kind: Pop, open: bool, since: Option<Instant>) -> Option<PopFrame> {
        if reduced() {
            return open.then_some(PopFrame::SHOWN);
        }
        match (open, since) {
            (true, None) => Some(PopFrame::SHOWN),
            (false, None) => None,
            (true, Some(t0)) => {
                let e = ease_out(progress(t0, kind.enter()));
                Some(PopFrame { opacity: e, dy: kind.enter_dy() * (1. - e), leaving: false })
            }
            (false, Some(t0)) => {
                let p = progress(t0, DURATION_EXIT);
                (p < 1.).then(|| {
                    let e = ease_in(p);
                    PopFrame { opacity: 1. - e, dy: kind.exit_dy() * e, leaving: true }
                })
            }
        }
    }

    /// At rest, nothing left to animate.
    pub fn settled(&self) -> bool {
        self.opacity >= 1. && self.dy == 0. && !self.leaving
    }
}

/// Open or closed and when it last changed, kept per popup between frames.
#[derive(Debug, Default)]
pub struct Presence {
    open: bool,
    since: Option<Instant>,
}

impl Presence {
    /// Record the popup's open state; a change restarts the clock.
    pub fn set(&mut self, open: bool) {
        if open != self.open {
            self.open = open;
            self.since = Some(Instant::now());
        }
    }

    /// The frame to draw now.
    pub fn frame(&self, kind: Pop) -> Option<PopFrame> {
        PopFrame::at(kind, self.open, self.since)
    }
}

/// Track popup `key` (unique within the view) through `open` changes. Call it every render
/// whether or not the popup is open: it returns the frame to draw, or `None` once the popup
/// is closed and its exit has played. Requests frames while animating.
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
pub fn pop_below(anchor: Div, gap: f32, f: PopFrame) -> Div {
    anchor.pt(px((gap + f.dy).max(0.))).opacity(f.opacity)
}

/// Position wrapper of a popover above its trigger (`gap` is the bottom padding).
pub fn pop_above(anchor: Div, gap: f32, f: PopFrame) -> Div {
    anchor.pb(px((gap - f.dy).max(0.))).opacity(f.opacity)
}

/// Enter-only popup animation for an element that is mounted while open (for popups whose
/// owner does not track presence): fades in and travels like `kind` over its enter
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

/// Appear-only animation for a confirm or banner revealed inside an open popup (180 ms,
/// 4px down). Plays each time the element is mounted.
pub fn reveal(id: impl Into<ElementId>, el: impl IntoElement) -> gpui::AnyElement {
    let el = div().flex().flex_col().child(el);
    if reduced() {
        return el.into_any_element();
    }
    let anim = Animation::new(scaled(DURATION_ENTER)).with_easing(ease_out);
    el.with_animation(id, anim, |d, t| d.opacity(t).mt(px(-4. * (1. - t)))).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn travel_and_timing_follow_the_motion_table() {
        assert_eq!(Pop::Below.enter(), Duration::from_millis(180));
        assert_eq!(Pop::Dialog.enter(), Duration::from_millis(220));
        assert_eq!(Pop::Toast.enter(), Duration::from_millis(240));
        assert_eq!(Pop::Below.enter_dy().abs(), 4.);
        assert_eq!(Pop::Dialog.enter_dy(), 8.);
        assert!(Pop::Toast.exit_dy().abs() <= 2.);
    }

    #[test]
    fn presence_remembers_when_it_changed() {
        let mut p = Presence::default();
        assert_eq!(p.frame(Pop::Below), None);
        p.set(true);
        assert!(p.frame(Pop::Below).is_some());
        p.set(false);
        if !reduced() {
            // Just closed: still on screen, leaving.
            assert!(p.frame(Pop::Below).is_some_and(|f| f.leaving));
        }
    }
}
