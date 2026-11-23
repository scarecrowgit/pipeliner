//! State fades: hover and selection colours ease over `DURATION_STATE` instead of snapping.

use std::time::Instant;

use gpui::{App, ElementId, Entity, Hsla, Stateful, Window, prelude::*};

use super::{DURATION_STATE, ease_standard, mix, progress, reduced};
use crate::theme::{SURFACE_HOVER, TRANSPARENT};

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
        self.from != self.to && !reduced() && progress(self.since, DURATION_STATE) < 1.
    }
}

/// A number easing the same way, for things that move (a toggle's knob).
#[derive(Clone, Copy, Debug)]
struct ValueFade {
    from: f32,
    to: f32,
    since: Instant,
}

impl ValueFade {
    fn value(&self) -> f32 {
        if reduced() {
            return self.to;
        }
        self.from + (self.to - self.from) * ease_standard(progress(self.since, DURATION_STATE))
    }

    fn animating(&self) -> bool {
        self.from != self.to && !reduced() && progress(self.since, DURATION_STATE) < 1.
    }
}

/// Hover flag plus up to three fading colours (background, text, border) and one fading
/// number of one element.
#[derive(Debug, Default)]
pub struct StateFade {
    /// The pointer is over the element.
    pub hovered: bool,
    colors: [Option<ColorFade>; 3],
    value: Option<ValueFade>,
}

impl StateFade {
    /// Ease slot `slot` toward `target`; returns the colour to draw now.
    fn toward(&mut self, slot: usize, target: Hsla) -> Hsla {
        match &mut self.colors[slot] {
            Some(f) if f.to == target => f.value(),
            Some(f) => {
                let now = f.value();
                *f = ColorFade { from: now, to: target, since: Instant::now() };
                now
            }
            None => {
                // First frame: start at the target, no fade.
                self.colors[slot] = Some(ColorFade { from: target, to: target, since: Instant::now() });
                target
            }
        }
    }

    fn value_toward(&mut self, target: f32) -> f32 {
        match &mut self.value {
            Some(f) if f.to == target => f.value(),
            Some(f) => {
                let now = f.value();
                *f = ValueFade { from: now, to: target, since: Instant::now() };
                now
            }
            None => {
                self.value = Some(ValueFade { from: target, to: target, since: Instant::now() });
                target
            }
        }
    }

    fn animating(&self) -> bool {
        self.colors.iter().flatten().any(ColorFade::animating)
            || self.value.is_some_and(|v| v.animating())
    }
}

/// Colours of an element at rest or when hovered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Background.
    pub bg: Hsla,
    /// Text colour, when it changes.
    pub fg: Option<Hsla>,
    /// Border colour, when it changes.
    pub border: Option<Hsla>,
}

impl Look {
    /// A look with only a background.
    pub fn bg(bg: Hsla) -> Self {
        Self { bg, fg: None, border: None }
    }

    /// Add a text colour.
    pub fn fg(mut self, fg: Hsla) -> Self {
        self.fg = Some(fg);
        self
    }

    /// Add a border colour.
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

/// The colours to draw this frame, from [`hover_fade_state`]. Use `fg` to tint an icon
/// inside the element so it fades together with the element.
pub struct HoverFade {
    state: Entity<StateFade>,
    /// Background to draw now.
    pub bg: Hsla,
    /// Text colour to draw now (when the looks set one).
    pub fg: Option<Hsla>,
    /// Border colour to draw now (when the looks set one).
    pub border: Option<Hsla>,
}

impl HoverFade {
    /// Apply the colours and the hover listener to `el`.
    pub fn apply(self, el: Stateful<gpui::Div>) -> Stateful<gpui::Div> {
        let mut el = el.bg(self.bg);
        if let Some(fg) = self.fg {
            el = el.text_color(fg);
        }
        if let Some(b) = self.border {
            el = el.border_color(b);
        }
        let state = self.state;
        el.on_hover(move |h, _, cx| {
            state.update(cx, |s, cx| {
                if s.hovered != *h {
                    s.hovered = *h;
                    cx.notify();
                }
            })
        })
    }
}

/// Resolve the fading colours of element `key` for this frame: `rest`, or `hover` while the
/// pointer is over it, eased over 150ms (also across later changes of `rest`, e.g. selection).
/// `key` must be unique within the view; use the element's own id.
pub fn hover_fade_state(
    key: impl Into<ElementId>,
    rest: Look,
    hover: Look,
    window: &mut Window,
    cx: &mut App,
) -> HoverFade {
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
    HoverFade { state, bg, fg, border }
}

/// Give `el` the colours of `rest`, or `hover` while the pointer is over it, fading between
/// them. See [`hover_fade_state`].
pub fn hover_fade(
    el: Stateful<gpui::Div>,
    key: impl Into<ElementId>,
    rest: Look,
    hover: Look,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<gpui::Div> {
    hover_fade_state(key, rest, hover, window, cx).apply(el)
}

/// [`hover_fade`] as an element, for builders without a `Window` at hand: wrap a finished row
/// (`motion::fade(row, id, rest, hover)`) and the fade is applied when it renders.
#[derive(IntoElement)]
pub struct Fade {
    el: Stateful<gpui::Div>,
    key: ElementId,
    rest: Look,
    hover: Look,
}

/// Wrap `el` so it fades between `rest` and `hover` when rendered.
pub fn fade(el: Stateful<gpui::Div>, key: impl Into<ElementId>, rest: Look, hover: Look) -> Fade {
    Fade { el, key: key.into(), rest, hover }
}

/// Look pair of a menu or list row: `selected` stays on `surface-hover`, others go from
/// transparent to `hover_bg` on hover.
pub fn row_looks(selected: bool, hover_bg: Hsla) -> (Look, Look) {
    if selected {
        let l = Look::bg(SURFACE_HOVER);
        (l, l)
    } else {
        (Look::bg(TRANSPARENT), Look::bg(hover_bg))
    }
}

// A `Fade` chains like the row it wraps (children, styles and handlers go to the row).
impl ParentElement for Fade {
    fn extend(&mut self, elements: impl IntoIterator<Item = gpui::AnyElement>) {
        self.el.extend(elements)
    }
}

impl gpui::Styled for Fade {
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

/// A menu or list row: `selected` sits on `surface-hover`, others fade to `hover_bg` on hover.
pub fn fade_row(
    el: Stateful<gpui::Div>,
    key: impl Into<ElementId>,
    selected: bool,
    hover_bg: Hsla,
) -> Fade {
    let (rest, hover) = row_looks(selected, hover_bg);
    fade(el, key, rest, hover)
}

impl RenderOnce for Fade {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        hover_fade(self.el, self.key, self.rest, self.hover, window, cx)
    }
}

/// Fade one colour (a tab label, an icon) toward `target` over 150ms.
pub fn fade_color(key: impl Into<ElementId>, target: Hsla, window: &mut Window, cx: &mut App) -> Hsla {
    let state: Entity<StateFade> = window.use_keyed_state(key, cx, |_, _| StateFade::default());
    let (c, animating) = state.update(cx, |s, _| (s.toward(0, target), s.animating()));
    if animating {
        window.request_animation_frame();
    }
    c
}

/// Fade one number (a toggle knob's position, 0 to 1) toward `target` over 150ms.
pub fn fade_value(key: impl Into<ElementId>, target: f32, window: &mut Window, cx: &mut App) -> f32 {
    let state: Entity<StateFade> = window.use_keyed_state(key, cx, |_, _| StateFade::default());
    let (v, animating) = state.update(cx, |s, _| (s.value_toward(target), s.animating()));
    if animating {
        window.request_animation_frame();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ACCENT, SURFACE_RAISED};

    #[test]
    fn a_fade_starts_at_the_target_and_eases_on_change() {
        let mut s = StateFade::default();
        assert_eq!(s.toward(0, SURFACE_RAISED), SURFACE_RAISED);
        assert!(!s.animating());
        // Changing the target starts from where the colour was.
        let now = s.toward(0, ACCENT);
        assert_eq!(now, SURFACE_RAISED);
        if !reduced() {
            assert!(s.animating());
        }
    }

    #[test]
    fn a_value_fade_starts_at_the_target() {
        let mut s = StateFade::default();
        assert_eq!(s.value_toward(0.), 0.);
        assert_eq!(s.value_toward(1.), 0.);
    }

    #[test]
    fn selected_rows_do_not_change_on_hover() {
        let (rest, hover) = row_looks(true, SURFACE_RAISED);
        assert_eq!(rest, hover);
        let (rest, hover) = row_looks(false, SURFACE_RAISED);
        assert_eq!(rest.bg, TRANSPARENT);
        assert_eq!(hover.bg, SURFACE_RAISED);
    }
}
