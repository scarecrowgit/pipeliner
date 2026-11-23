//! The elements of a rendered document: [`UnitElement`] lays out, paints and registers one
//! selectable text unit; [`Tracker`], painted after all of them, owns the mouse: selection
//! across units, link hover and link clicks.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, CursorStyle, DispatchPhase, Element, ElementId, FocusHandle,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Position, Style,
    WeakEntity, Window, fill, px, relative, size,
};
use ui::theme::{ACCENT, with_alpha};

use super::layout::{InlineLayout, Look};
use super::rich::RichText;
use super::selection::{Selection, TextPoint, word_range};
use super::view::{LinkHandler, Markdown};

/// A unit as laid out this frame.
pub(crate) struct FrameUnit {
    pub unit: usize,
    pub bounds: Bounds<Pixels>,
    pub layout: Rc<InlineLayout>,
    pub hitbox: Hitbox,
}

/// Mouse and selection state shared by a document's elements and its view.
#[derive(Default)]
pub(crate) struct Shared {
    pub sel: Selection,
    pub dragging: bool,
    /// Units registered this frame, in paint (reading) order.
    pub frame: Vec<FrameUnit>,
    /// (unit, link) under the mouse.
    pub hover_link: Option<(usize, usize)>,
    /// (unit, link) the mouse went down on.
    pub pressed_link: Option<(usize, usize)>,
}

impl Shared {
    /// Where a drag at `pos` puts the selection head: the unit under it, else the unit
    /// level with it, else the end of the last unit above it (or the start of the first).
    fn point_at(&self, pos: Point<Pixels>) -> Option<TextPoint> {
        let at = |f: &FrameUnit| TextPoint {
            unit: f.unit,
            offset: f.layout.offset_at(pos - f.bounds.origin),
        };
        if let Some(f) = self.frame.iter().find(|f| f.bounds.contains(&pos)) {
            return Some(at(f));
        }
        let level = self
            .frame
            .iter()
            .filter(|f| f.bounds.top() <= pos.y && pos.y < f.bounds.bottom())
            .min_by(|a, b| h_dist(a, pos).total_cmp(&h_dist(b, pos)));
        if let Some(f) = level {
            return Some(at(f));
        }
        let above = self
            .frame
            .iter()
            .filter(|f| f.bounds.bottom() <= pos.y)
            .max_by(|a, b| {
                f32::from(a.bounds.bottom())
                    .total_cmp(&f32::from(b.bounds.bottom()))
                    .then(a.unit.cmp(&b.unit))
            });
        match above {
            Some(f) => Some(TextPoint {
                unit: f.unit,
                offset: f.layout.rich.text.len(),
            }),
            None => self.frame.iter().min_by_key(|f| f.unit).map(|f| TextPoint {
                unit: f.unit,
                offset: 0,
            }),
        }
    }

    fn hovered(&self, window: &Window) -> Option<&FrameUnit> {
        self.frame.iter().find(|f| f.hitbox.is_hovered(window))
    }
}

fn h_dist(f: &FrameUnit, pos: Point<Pixels>) -> f32 {
    let x = f32::from(pos.x);
    let (l, r) = (f32::from(f.bounds.left()), f32::from(f.bounds.right()));
    if x < l {
        l - x
    } else if x > r {
        x - r
    } else {
        0.
    }
}

type LayoutCache = Rc<RefCell<Option<(Option<f32>, Rc<InlineLayout>)>>>;

/// One selectable text unit.
pub(crate) struct UnitElement {
    unit: usize,
    rich: Rc<RichText>,
    look: Look,
    shared: Rc<RefCell<Shared>>,
    focused: bool,
}

impl UnitElement {
    pub fn new(
        unit: usize,
        rich: Rc<RichText>,
        look: Look,
        shared: Rc<RefCell<Shared>>,
        focused: bool,
    ) -> Self {
        Self {
            unit,
            rich,
            look,
            shared,
            focused,
        }
    }

    fn hover(&self) -> Option<Range<usize>> {
        let (u, l) = self.shared.borrow().hover_link?;
        (u == self.unit)
            .then(|| self.rich.links.get(l).map(|(r, _)| r.clone()))
            .flatten()
    }

    fn wrap_width(look: &Look, avail: Option<Pixels>) -> Option<f32> {
        if !look.wrap {
            return None;
        }
        match (avail.map(f32::from), look.max_width) {
            (Some(a), Some(m)) => Some(a.min(m)),
            (a, m) => a.or(m),
        }
    }
}

impl IntoElement for UnitElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for UnitElement {
    type RequestLayoutState = LayoutCache;
    type PrepaintState = Option<(Rc<InlineLayout>, Hitbox)>;

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
        _: &mut App,
    ) -> (LayoutId, LayoutCache) {
        let mut style = Style {
            flex_shrink: 0.,
            ..Default::default()
        };
        if self.look.fill {
            style.size.width = relative(1.).into();
        }
        let cache: LayoutCache = Rc::default();
        let (c, rich, look, hover) = (
            cache.clone(),
            self.rich.clone(),
            self.look.clone(),
            self.hover(),
        );
        let id = window.request_measured_layout(style, move |known, available, window, _| {
            let avail = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let wrap = Self::wrap_width(&look, avail);
            let layout = InlineLayout::build(rich.clone(), &look, hover.clone(), wrap, window);
            let sz = size(
                known.width.unwrap_or(px(layout.width().ceil())),
                known.height.unwrap_or(px(layout.height())),
            );
            *c.borrow_mut() = Some((wrap, Rc::new(layout)));
            sz
        });
        (id, cache)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        cache: &mut LayoutCache,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let wrap = Self::wrap_width(&self.look, Some(bounds.size.width));
        let cached = cache.borrow().as_ref().and_then(|(w, l)| {
            let same = match (w, wrap) {
                (Some(a), Some(b)) => (a - b).abs() < 0.5,
                (None, None) => true,
                _ => false,
            };
            same.then(|| l.clone())
        });
        let layout = cached.unwrap_or_else(|| {
            Rc::new(InlineLayout::build(
                self.rich.clone(),
                &self.look,
                self.hover(),
                wrap,
                window,
            ))
        });
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.shared.borrow_mut().frame.push(FrameUnit {
            unit: self.unit,
            bounds,
            layout: layout.clone(),
            hitbox: hitbox.clone(),
        });
        Some((layout, hitbox))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut LayoutCache,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((layout, hitbox)) = state.as_ref() else {
            return;
        };
        let (range, link_hovered) = {
            let s = self.shared.borrow();
            let range = s.sel.range_in(self.unit, self.rich.text.len());
            (range, s.hover_link.is_some_and(|(u, _)| u == self.unit))
        };
        if let Some(r) = range {
            let tint = with_alpha(ACCENT, if self.focused { 0.32 } else { 0.18 });
            for rect in layout.selection_rects(r) {
                window.paint_quad(fill(
                    Bounds::new(bounds.origin + rect.origin, rect.size),
                    tint,
                ));
            }
        }
        let clip = window.content_mask().bounds;
        layout.paint(bounds.origin, clip, window, cx);
        let cursor = if link_hovered {
            CursorStyle::PointingHand
        } else {
            CursorStyle::IBeam
        };
        window.set_cursor_style(cursor, hitbox);
    }
}

/// Zero-size element painted after every unit of a document; registers its mouse handlers.
pub(crate) struct Tracker {
    pub shared: Rc<RefCell<Shared>>,
    pub view: WeakEntity<Markdown>,
    pub focus: FocusHandle,
    pub on_link: Option<LinkHandler>,
}

impl IntoElement for Tracker {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn notify(view: &WeakEntity<Markdown>, cx: &mut App) {
    if let Some(v) = view.upgrade() {
        v.update(cx, |_, cx| cx.notify());
    }
}

impl Element for Tracker {
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
        let style = Style {
            position: Position::Absolute,
            size: size(px(0.).into(), px(0.).into()),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let (shared, view, focus) = (self.shared.clone(), self.view.clone(), self.focus.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            let mut s = shared.borrow_mut();
            let Some(f) = s.hovered(window) else {
                let had = !s.sel.is_empty();
                s.sel.clear();
                s.pressed_link = None;
                drop(s);
                if had {
                    notify(&view, cx);
                }
                return;
            };
            let (unit, local, layout) = (f.unit, e.position - f.bounds.origin, f.layout.clone());
            let text = &layout.rich.text;
            let offset = layout.offset_at(local);
            match e.click_count {
                2 => {
                    let at = layout.offset_under(local).unwrap_or(offset);
                    s.sel.set(unit, word_range(text, at));
                }
                n if n >= 3 => s.sel.set(unit, 0..text.len()),
                _ if e.modifiers.shift && s.sel.anchor.is_some() => {
                    s.sel.head = Some(TextPoint { unit, offset });
                }
                _ => s.sel.collapse(TextPoint { unit, offset }),
            }
            s.dragging = e.click_count <= 1;
            s.pressed_link = (e.click_count <= 1 && !e.modifiers.shift)
                .then(|| layout.link_under(local).map(|l| (unit, l)))
                .flatten();
            drop(s);
            window.focus(&focus);
            notify(&view, cx);
        });

        let (shared, view) = (self.shared.clone(), self.view.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let mut s = shared.borrow_mut();
            if s.dragging && e.pressed_button == Some(MouseButton::Left) {
                let head = s.point_at(e.position);
                if head.is_some() && head != s.sel.head {
                    s.sel.head = head;
                    drop(s);
                    notify(&view, cx);
                }
                return;
            }
            let hover = s.hovered(window).and_then(|f| {
                f.layout
                    .link_under(e.position - f.bounds.origin)
                    .map(|l| (f.unit, l))
            });
            if hover != s.hover_link {
                s.hover_link = hover;
                drop(s);
                notify(&view, cx);
            }
        });

        let (shared, on_link) = (self.shared.clone(), self.on_link.clone());
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            let mut s = shared.borrow_mut();
            s.dragging = false;
            let Some((unit, link)) = s.pressed_link.take() else {
                return;
            };
            let url = s
                .hovered(window)
                .filter(|f| f.unit == unit && s.sel.is_empty())
                .filter(|f| f.layout.link_under(e.position - f.bounds.origin) == Some(link))
                .and_then(|f| f.layout.rich.links.get(link).map(|(_, u)| u.clone()));
            drop(s);
            if let (Some(url), Some(f)) = (url, on_link.as_ref()) {
                f(&url, window, cx);
            }
        });
    }
}

/// Origin helper for tests: the window position of byte `offset` of unit `unit`, if it was
/// laid out this frame.
#[cfg(test)]
pub(crate) fn unit_point(shared: &Shared, unit: usize, offset: usize) -> Option<Point<Pixels>> {
    let f = shared.frame.iter().find(|f| f.unit == unit)?;
    let r = f
        .layout
        .selection_rects(offset..offset + 1)
        .into_iter()
        .next()?;
    Some(f.bounds.origin + r.origin + gpui::point(px(1.), r.size.height / 2.))
}
