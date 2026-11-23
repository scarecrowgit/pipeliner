//! The canvas viewport: zoom and pan, and the conversions between canvas units and screen
//! pixels.
//!
//! Screen position = `origin` + canvas position × `zoom`, where `origin` is where canvas
//! `(0, 0)` lands, in pixels from the top-left of the canvas element. GPUI has no element
//! transforms, so every drawn length is multiplied by the zoom (see [`Viewport::z`]).

use gpui::{Bounds, Pixels, Point, Size, point, px, size};

/// The smallest zoom.
pub const MIN_ZOOM: f32 = 0.25;
/// The largest zoom.
pub const MAX_ZOOM: f32 = 2.0;
/// The factor one press of the toolbar's zoom buttons applies.
pub const ZOOM_STEP: f32 = 1.1;

/// `units` at `zoom`, in pixels. Non-finite results become zero, so a bad zoom cannot reach
/// the layout engine.
pub fn z(zoom: f32, units: f32) -> Pixels {
    let v = units * zoom;
    px(if v.is_finite() { v } else { 0.0 })
}

fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() { zoom.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 }
}

/// Zoom and pan of the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Pixels per canvas unit, `0.25..=2.0`.
    pub zoom: f32,
    /// Where canvas `(0, 0)` lands, in pixels from the canvas element's top-left.
    pub origin: Point<f32>,
}

impl Default for Viewport {
    fn default() -> Self {
        Self { zoom: 1.0, origin: point(0.0, 0.0) }
    }
}

impl Viewport {
    /// A viewport at `zoom` (clamped to the range) with canvas `(0, 0)` at `origin`.
    pub fn new(zoom: f32, origin: Point<f32>) -> Self {
        Self { zoom: clamp_zoom(zoom), origin }.sanitized()
    }

    /// The same viewport with a zoom inside the range and a finite origin: what painting
    /// uses, so fields set directly cannot break it.
    pub fn sanitized(self) -> Self {
        let fix = |v: f32| if v.is_finite() { v } else { 0.0 };
        Self { zoom: clamp_zoom(self.zoom), origin: point(fix(self.origin.x), fix(self.origin.y)) }
    }

    /// `units` at this zoom, as a pixel length. Use it for every size, radius, stroke and
    /// font size of a zoomed element.
    pub fn z(&self, units: f32) -> Pixels {
        z(self.zoom, units)
    }

    /// `units` at this zoom as a plain number.
    pub fn scale(&self, units: f32) -> f32 {
        units * self.zoom
    }

    /// The zoom as the toolbar shows it: `0.86` is `86`.
    pub fn percent(&self) -> u32 {
        (clamp_zoom(self.zoom) * 100.0).round() as u32
    }

    /// A canvas position on screen.
    pub fn to_screen(&self, p: Point<f32>) -> Point<f32> {
        point(self.origin.x + p.x * self.zoom, self.origin.y + p.y * self.zoom)
    }

    /// A screen position (pixels from the canvas element's top-left) in canvas units.
    pub fn to_canvas(&self, p: Point<f32>) -> Point<f32> {
        let zoom = clamp_zoom(self.zoom);
        point((p.x - self.origin.x) / zoom, (p.y - self.origin.y) / zoom)
    }

    /// A pointer position in window pixels, given where the canvas element sits in the
    /// window (its bounds' origin), in canvas units.
    pub fn pointer_to_canvas(&self, pointer: Point<Pixels>, canvas_origin: Point<Pixels>) -> Point<f32> {
        self.to_canvas(point(
            f32::from(pointer.x) - f32::from(canvas_origin.x),
            f32::from(pointer.y) - f32::from(canvas_origin.y),
        ))
    }

    /// The part of the canvas that is on screen in an element of `viewport_size`, in canvas
    /// units.
    pub fn visible(&self, viewport_size: Size<f32>) -> Bounds<f32> {
        let zoom = clamp_zoom(self.zoom);
        Bounds::new(
            self.to_canvas(point(0.0, 0.0)),
            size(viewport_size.width / zoom, viewport_size.height / zoom),
        )
    }

    /// Zoom by `factor` (clamped to the range) keeping the canvas point under the screen
    /// position `at` where it is. Does nothing for a factor that is not a positive number.
    pub fn zoom_at(&mut self, at: Point<f32>, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.set_zoom_at(at, self.zoom * factor);
    }

    /// Set the zoom (clamped to the range), keeping the canvas point under `at` fixed.
    pub fn set_zoom_at(&mut self, at: Point<f32>, zoom: f32) {
        *self = self.sanitized();
        let new = clamp_zoom(zoom);
        let ratio = new / self.zoom;
        self.origin = point(at.x - (at.x - self.origin.x) * ratio, at.y - (at.y - self.origin.y) * ratio);
        self.zoom = new;
    }

    /// Move the canvas by `delta` pixels.
    pub fn pan(&mut self, delta: Point<f32>) {
        if delta.x.is_finite() && delta.y.is_finite() {
            self.origin = point(self.origin.x + delta.x, self.origin.y + delta.y);
        }
    }

    /// The viewport that shows all of `content` centred in an element of `viewport_size`,
    /// with `padding` pixels around it.
    ///
    /// The zoom is clamped to the range, so small content is not blown up past 200 %. Empty
    /// content is centred at zoom 1. A viewport too small for the padding gets the minimum
    /// zoom.
    pub fn fit(content: Bounds<f32>, viewport_size: Size<f32>, padding: f32) -> Viewport {
        let pad = if padding.is_finite() { padding.max(0.0) } else { 0.0 };
        let avail_w = (viewport_size.width - 2.0 * pad).max(0.0);
        let avail_h = (viewport_size.height - 2.0 * pad).max(0.0);
        let (cw, ch) = (content.size.width.max(0.0), content.size.height.max(0.0));
        let empty = cw <= f32::EPSILON || ch <= f32::EPSILON;
        let zoom = if empty {
            1.0
        } else {
            clamp_zoom((avail_w / cw).min(avail_h / ch))
        };
        let center = point(
            content.origin.x + content.size.width / 2.0,
            content.origin.y + content.size.height / 2.0,
        );
        Viewport::new(
            zoom,
            point(
                viewport_size.width / 2.0 - center.x * zoom,
                viewport_size.height / 2.0 - center.y * zoom,
            ),
        )
    }
}
