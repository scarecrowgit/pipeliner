//! Where a popover goes: the trigger's bounds ([`Anchor`]), the side and alignment it opens
//! on ([`Placement`]), and the element that keeps it inside the window.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Display, Element, ElementId, Global, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, Position, Size, Style, Window,
    WindowId, point, px, size,
};

use crate::theme::{SPACE_1, SPACE_2};

/// Bounds of the triggers that opted in with [`PopoverAnchorExt::popover_anchor`], per
/// window, as of the last frame that drew them.
#[derive(Default)]
struct AnchorRegistry(HashMap<(WindowId, ElementId), Bounds<Pixels>>);

impl Global for AnchorRegistry {}

fn recorded(key: &ElementId, window: &Window, cx: &App) -> Option<Bounds<Pixels>> {
    let id = window.window_handle().window_id();
    cx.try_global::<AnchorRegistry>()?.0.get(&(id, key.clone())).copied()
}

/// What a popover is anchored to: the trigger's bounds in window coordinates, captured when
/// it was pressed. With a key (from [`Anchor::of`]) the popover follows the trigger if it
/// moves while open.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    /// The trigger's bounds.
    pub bounds: Bounds<Pixels>,
    pub(crate) key: Option<ElementId>,
}

impl Anchor {
    /// The bounds of the trigger marked `.popover_anchor(key)`. Falls back to the pointer
    /// position when that trigger has not been drawn.
    pub fn of(key: impl Into<ElementId>, window: &Window, cx: &App) -> Anchor {
        let key = key.into();
        let bounds = recorded(&key, window, cx)
            .unwrap_or_else(|| Bounds::new(window.mouse_position(), size(px(0.), px(0.))));
        Anchor { bounds, key: Some(key) }
    }

    /// Anchored to fixed bounds.
    pub fn bounds(bounds: Bounds<Pixels>) -> Anchor {
        Anchor { bounds, key: None }
    }

    /// Anchored to a point (a context menu at the pointer).
    pub fn point(at: Point<Pixels>) -> Anchor {
        Anchor::bounds(Bounds::new(at, size(px(0.), px(0.))))
    }

    /// From a click: the pressed element's bounds when it came from the keyboard, the
    /// pointer position when it came from the mouse.
    pub fn from_click(event: &ClickEvent) -> Anchor {
        match event {
            ClickEvent::Keyboard(k) => Anchor::bounds(k.bounds),
            ClickEvent::Mouse(_) => Anchor::point(event.position()),
        }
    }

    /// The key the anchor was looked up by, if any.
    pub fn key(&self) -> Option<&ElementId> {
        self.key.as_ref()
    }
}

/// Which side of the trigger a popover opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Side {
    /// Under the trigger, growing down (the default).
    #[default]
    Below,
    /// Over the trigger, growing up: triggers near the bottom of the window.
    Above,
}

/// How a popover lines up with its trigger along the side it opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    /// Left edges line up (the default).
    #[default]
    Start,
    /// Centres line up.
    Center,
    /// Right edges line up: triggers on the right edge of a header.
    End,
}

/// Where a popover opens relative to its [`Anchor`]: a side, an alignment and a gap. It flips
/// to the other side when it does not fit, and shifts along the edge to stay in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The preferred side.
    pub side: Side,
    /// The alignment along that side.
    pub align: Align,
    /// Space between trigger and popover, px (4 by default).
    pub gap: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Placement::below()
    }
}

impl Placement {
    /// Under the trigger, left edges aligned, 4px apart.
    pub const fn below() -> Self {
        Placement { side: Side::Below, align: Align::Start, gap: SPACE_1 }
    }

    /// Over the trigger, left edges aligned, 4px apart.
    pub const fn above() -> Self {
        Placement { side: Side::Above, align: Align::Start, gap: SPACE_1 }
    }

    /// Align the left edges.
    pub const fn start(mut self) -> Self {
        self.align = Align::Start;
        self
    }

    /// Align the centres.
    pub const fn center(mut self) -> Self {
        self.align = Align::Center;
        self
    }

    /// Align the right edges.
    pub const fn end(mut self) -> Self {
        self.align = Align::End;
        self
    }

    /// Set the gap to the trigger, px.
    pub const fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }
}

/// The smallest distance a popover keeps from the window's edges, px.
pub const WINDOW_MARGIN: f32 = SPACE_2;

/// Top-left corner of a popover of `size` placed at `anchor` in a window of `viewport`, and
/// the side it ended up on. Flips when the preferred side does not fit and the other does
/// (or has more room); shifts along the edge so it stays inside the window.
pub fn place(
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    viewport: Size<Pixels>,
    p: Placement,
) -> (Point<Pixels>, Side) {
    let f = |v: Pixels| f32::from(v);
    let (w, h) = (f(size.width), f(size.height));
    let (vw, vh) = (f(viewport.width), f(viewport.height));
    let (top, bottom) = (f(anchor.top()), f(anchor.bottom()));
    let (left, right) = (f(anchor.left()), f(anchor.right()));
    let m = WINDOW_MARGIN;

    let below_y = bottom + p.gap;
    let above_y = top - p.gap - h;
    let fits_below = below_y + h <= vh - m;
    let fits_above = above_y >= m;
    let side = match p.side {
        Side::Below if fits_below => Side::Below,
        Side::Above if fits_above => Side::Above,
        Side::Below if fits_above => Side::Above,
        Side::Above if fits_below => Side::Below,
        // Neither fits: the side with more room, clamped below.
        _ => {
            if vh - bottom >= top { Side::Below } else { Side::Above }
        }
    };
    let y = match side {
        Side::Below => below_y,
        Side::Above => above_y,
    };
    let y = y.min(vh - m - h).max(m);

    let x = match p.align {
        Align::Start => left,
        Align::Center => left + (right - left - w) / 2.,
        Align::End => right - w,
    };
    let x = x.min(vw - m - w).max(m);
    (point(px(x), px(y)), side)
}

/// A trigger that records its bounds every frame under a key, for [`Anchor::of`]. Made by
/// [`PopoverAnchorExt::popover_anchor`]; it adds no layout of its own.
pub struct AnchorProbe {
    key: ElementId,
    child: AnyElement,
}

/// `.popover_anchor(key)` on any element: remember where it is so a popover can open from it.
pub trait PopoverAnchorExt: IntoElement + Sized {
    /// Record this element's bounds under `key` (unique in the window) for [`Anchor::of`].
    fn popover_anchor(self, key: impl Into<ElementId>) -> AnchorProbe {
        AnchorProbe { key: key.into(), child: self.into_any_element() }
    }
}

impl<E: IntoElement> PopoverAnchorExt for E {}

impl IntoElement for AnchorProbe {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AnchorProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let id = window.window_handle().window_id();
        cx.default_global::<AnchorRegistry>().0.insert((id, self.key.clone()), bounds);
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// A popover panel positioned by [`place`] once its size is known, moved `toward` px toward
/// its trigger (the enter and exit travel).
pub(crate) struct Placed {
    pub anchor: Anchor,
    pub placement: Placement,
    pub toward: f32,
    pub child: AnyElement,
}

impl IntoElement for Placed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Placed {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let child = self.child.request_layout(window, cx);
        let style = Style { position: Position::Absolute, display: Display::Flex, ..Style::default() };
        (window.request_layout(style, [child], cx), child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let size = window.layout_bounds(*child).size;
        let anchor = self
            .anchor
            .key
            .as_ref()
            .and_then(|k| recorded(k, window, cx))
            .unwrap_or(self.anchor.bounds);
        let (origin, side) = place(anchor, size, window.viewport_size(), self.placement);
        let travel = match side {
            Side::Below => -self.toward,
            Side::Above => self.toward,
        };
        let origin = point(origin.x, origin.y + px(travel));
        let offset = origin - bounds.origin;
        let offset = point(offset.x.round(), offset.y.round());
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    fn view() -> Size<Pixels> {
        size(px(1000.), px(800.))
    }

    #[test]
    fn opens_below_and_aligns_start_or_end() {
        let trigger = b(100., 10., 80., 40.);
        let (at, side) = place(trigger, size(px(300.), px(200.)), view(), Placement::below());
        assert_eq!((at, side), (point(px(100.), px(54.)), Side::Below));
        let (at, _) = place(trigger, size(px(60.), px(200.)), view(), Placement::below().end());
        assert_eq!(at.x, px(120.));
        let (at, _) = place(trigger, size(px(40.), px(20.)), view(), Placement::below().center());
        assert_eq!(at.x, px(120.));
    }

    #[test]
    fn flips_above_when_there_is_no_room_below_and_back() {
        let trigger = b(100., 700., 80., 40.);
        let (at, side) = place(trigger, size(px(300.), px(200.)), view(), Placement::below());
        assert_eq!(side, Side::Above);
        assert_eq!(at.y, px(700. - 4. - 200.));
        let top = b(100., 10., 80., 40.);
        let (_, side) = place(top, size(px(300.), px(200.)), view(), Placement::above());
        assert_eq!(side, Side::Below);
    }

    #[test]
    fn shifts_to_stay_inside_the_window() {
        let trigger = b(950., 10., 40., 40.);
        let (at, _) = place(trigger, size(px(300.), px(200.)), view(), Placement::below());
        assert_eq!(at.x, px(1000. - WINDOW_MARGIN - 300.));
        let left = b(0., 10., 40., 40.);
        let (at, _) = place(left, size(px(300.), px(200.)), view(), Placement::below().end());
        assert_eq!(at.x, px(WINDOW_MARGIN));
        // Taller than either side: the roomier side, clamped into the window.
        let mid = b(100., 300., 40., 40.);
        let (at, side) = place(mid, size(px(200.), px(700.)), view(), Placement::below());
        assert_eq!(side, Side::Below);
        assert_eq!(at.y, px(800. - WINDOW_MARGIN - 700.));
    }
}
