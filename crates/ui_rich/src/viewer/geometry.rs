//! Where the viewer draws its image: the zoom, the image's top-left inside the stage, and the
//! rules that keep both in range. GPUI-free, in plain `f32` logical pixels.
//!
//! The stage is the clipped area under the toolbar. `scale` 1 is 100%: one image pixel on one
//! screen pixel (the image's natural size divided by the window's scale factor).

/// Room kept between a fitted image and the stage's edges, in px.
pub const MARGIN: f32 = 24.0;
/// The largest zoom (800%).
pub const MAX_SCALE: f32 = 8.0;
/// The smallest zoom for an image that fits at a larger one (10%). A bigger image may go
/// down to its fit zoom.
pub const MIN_SCALE: f32 = 0.1;
/// The zooms `+` and `-` step through.
pub const STEPS: [f32; 13] = [0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0];

/// A width and a height in px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Extent {
    /// `w` × `h`, with anything not finite or below one pixel made one pixel.
    pub fn new(w: f32, h: f32) -> Self {
        let fix = |v: f32| if v.is_finite() { v.max(1.0) } else { 1.0 };
        Self { w: fix(w), h: fix(h) }
    }
}

/// What the zoom follows when the stage changes size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The whole image in the stage, at most 100%.
    Fit,
    /// The image as wide as the stage, read from the top.
    FitWidth,
    /// A zoom the user chose; kept when the window resizes.
    Free,
}

/// The view of one image: zoom and position, always inside the limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// The clipped area the image is drawn in.
    pub stage: Extent,
    /// The image at 100%.
    pub image: Extent,
    /// Zoom: 1 is 100%.
    pub scale: f32,
    /// Top-left of the image from the stage's top-left.
    pub x: f32,
    /// See `x`.
    pub y: f32,
    /// What the zoom follows.
    pub mode: Mode,
    /// Room kept under the image when it fits or is scrolled to its end (a caption sits
    /// there); at least [`MARGIN`].
    pub bottom: f32,
}

impl Viewport {
    /// `image` fitted into `stage`, keeping `bottom` px free under it (at least [`MARGIN`]).
    pub fn new(stage: Extent, image: Extent, bottom: f32) -> Self {
        let bottom = if bottom.is_finite() { bottom.max(MARGIN) } else { MARGIN };
        let mut v = Self { stage, image, scale: 1.0, x: 0.0, y: 0.0, mode: Mode::Fit, bottom };
        v.fit();
        v
    }

    /// Height the image may take without going under the toolbar margin or the bottom room.
    fn room_h(&self) -> f32 {
        (self.stage.h - MARGIN - self.bottom).max(1.0)
    }

    /// The zoom at which the whole image fits the stage with its margins, never above 100%.
    pub fn fit_scale(&self) -> f32 {
        let w = (self.stage.w - 2.0 * MARGIN).max(1.0) / self.image.w;
        let h = self.room_h() / self.image.h;
        w.min(h).min(1.0)
    }

    /// The zoom at which the image is as wide as the stage less its margins.
    pub fn fit_width_scale(&self) -> f32 {
        ((self.stage.w - 2.0 * MARGIN).max(1.0) / self.image.w).clamp(self.min_scale(), MAX_SCALE)
    }

    /// The smallest zoom allowed for this image in this stage.
    pub fn min_scale(&self) -> f32 {
        let w = (self.stage.w - 2.0 * MARGIN).max(1.0) / self.image.w;
        let h = self.room_h() / self.image.h;
        w.min(h).min(MIN_SCALE)
    }

    /// The zoom as the toolbar shows it: `0.86` is `86`.
    pub fn percent(&self) -> u32 {
        (self.scale * 100.0).round() as u32
    }

    /// The image's drawn size at the current zoom.
    pub fn drawn(&self) -> Extent {
        Extent { w: self.image.w * self.scale, h: self.image.h * self.scale }
    }

    /// The point `(px, py)` of the stage is on the image.
    pub fn hits_image(&self, px: f32, py: f32) -> bool {
        let d = self.drawn();
        px >= self.x && px <= self.x + d.w && py >= self.y && py <= self.y + d.h
    }

    /// Show the whole image, centred.
    pub fn fit(&mut self) {
        self.mode = Mode::Fit;
        self.scale = self.fit_scale();
        self.centre();
    }

    /// Make the image as wide as the stage and show its top.
    pub fn fit_width(&mut self) {
        self.mode = Mode::FitWidth;
        self.scale = self.fit_width_scale();
        self.x = f32::NEG_INFINITY;
        self.y = f32::INFINITY;
        self.clamp();
    }

    /// Fit, or 100% around the stage point `(ax, ay)` when it is fitted already: what a
    /// double-click does.
    pub fn toggle_actual(&mut self, ax: f32, ay: f32) {
        if self.mode == Mode::Fit && (self.scale - 1.0).abs() > 1e-3 {
            self.zoom_at(ax, ay, 1.0);
        } else if self.mode == Mode::Fit {
            // Already at 100% as its fit: nothing to toggle to.
        } else {
            self.fit();
        }
    }

    /// Zoom to `scale` (clamped) keeping the image point under the stage point `(ax, ay)`
    /// where it is.
    pub fn zoom_at(&mut self, ax: f32, ay: f32, scale: f32) {
        let scale = if scale.is_finite() { scale } else { 1.0 };
        let new = scale.clamp(self.min_scale(), MAX_SCALE);
        let ratio = new / self.scale;
        self.x = ax - (ax - self.x) * ratio;
        self.y = ay - (ay - self.y) * ratio;
        self.scale = new;
        self.mode = Mode::Free;
        self.clamp();
    }

    /// Zoom to `scale` around the stage's centre.
    pub fn zoom_to(&mut self, scale: f32) {
        self.zoom_at(self.stage.w / 2.0, self.stage.h / 2.0, scale);
    }

    /// One step of [`STEPS`] in (`up`) or out, around the stage's centre.
    pub fn step(&mut self, up: bool) {
        let s = self.scale;
        let next = if up {
            STEPS.iter().copied().find(|v| *v > s * 1.01).unwrap_or(MAX_SCALE)
        } else {
            STEPS.iter().copied().rev().find(|v| *v < s * 0.99).unwrap_or(self.min_scale())
        };
        self.zoom_to(next);
    }

    /// Move the image by `(dx, dy)`, as far as the limits allow.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        self.x += dx;
        self.y += dy;
        if self.mode == Mode::Fit {
            self.mode = Mode::Free;
        }
        self.clamp();
    }

    /// The stage changed size: refit in a fit mode, else keep the zoom and the limits.
    pub fn resize(&mut self, stage: Extent) {
        if stage == self.stage {
            return;
        }
        let (cx, cy) = (self.stage.w / 2.0, self.stage.h / 2.0);
        self.x += stage.w / 2.0 - cx;
        self.y += stage.h / 2.0 - cy;
        self.stage = stage;
        match self.mode {
            Mode::Fit => self.fit(),
            Mode::FitWidth => {
                let top = self.y;
                self.scale = self.fit_width_scale();
                self.y = top;
                self.clamp();
            }
            Mode::Free => {
                self.scale = self.scale.clamp(self.min_scale(), MAX_SCALE);
                self.clamp();
            }
        }
    }

    fn centre(&mut self) {
        let d = self.drawn();
        self.x = (self.stage.w - d.w) / 2.0;
        self.y = (self.stage.h - d.h) / 2.0;
        self.clamp();
    }

    /// Keep the image in view: an axis the image fits on is centred; on an axis it is larger,
    /// its edges stay at most [`MARGIN`] (the bottom: `bottom`) inside the stage's.
    fn clamp(&mut self) {
        let d = self.drawn();
        self.x = clamp_axis(self.x, d.w, self.stage.w, MARGIN, MARGIN);
        self.y = clamp_axis(self.y, d.h, self.stage.h, MARGIN, self.bottom);
    }
}

/// `pos` on an axis of length `stage` with `start` and `end` px kept free at its ends.
fn clamp_axis(pos: f32, size: f32, stage: f32, start: f32, end: f32) -> f32 {
    let room = stage - start - end;
    if size <= room {
        return start + (room - size) / 2.0;
    }
    let (lo, hi) = (stage - end - size, start);
    if pos.is_nan() { hi } else { pos.clamp(lo, hi) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tall() -> Viewport {
        // A 1440 × 6000 page render in a 1200 × 800 stage.
        Viewport::new(Extent::new(1200.0, 800.0), Extent::new(1440.0, 6000.0), MARGIN)
    }

    #[test]
    fn fit_shows_the_whole_image_centred_and_never_enlarges() {
        let v = tall();
        assert!((v.drawn().h - (800.0 - 2.0 * MARGIN)).abs() < 0.01);
        assert!((v.x - (1200.0 - v.drawn().w) / 2.0).abs() < 0.01);
        let small = Viewport::new(Extent::new(1200.0, 800.0), Extent::new(100.0, 50.0), MARGIN);
        assert_eq!(small.scale, 1.0, "a small image stays at 100%");
        assert_eq!((small.x, small.y), (550.0, 375.0));
    }

    #[test]
    fn fit_width_starts_at_the_top() {
        let mut v = tall();
        v.fit_width();
        assert_eq!(v.mode, Mode::FitWidth);
        assert!((v.drawn().w - (1200.0 - 2.0 * MARGIN)).abs() < 0.01);
        assert_eq!(v.y, MARGIN, "the top of the page, below the margin");
        assert!((v.x - MARGIN).abs() < 0.01);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let mut v = tall();
        v.zoom_to(1.0);
        let (ax, ay) = (300.0, 400.0);
        let (ix, iy) = ((ax - v.x) / v.scale, (ay - v.y) / v.scale);
        v.zoom_at(ax, ay, 2.0);
        assert!(((ax - v.x) / v.scale - ix).abs() < 0.01);
        assert!(((ay - v.y) / v.scale - iy).abs() < 0.01);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut v = tall();
        v.zoom_to(100.0);
        assert_eq!(v.scale, MAX_SCALE);
        v.zoom_to(0.0001);
        assert!((v.scale - v.min_scale()).abs() < 1e-6);
        assert!(v.min_scale() <= v.fit_scale());
        v.zoom_to(f32::NAN);
        assert_eq!(v.scale, 1.0);
        for _ in 0..40 {
            v.step(true);
        }
        assert_eq!(v.scale, MAX_SCALE);
        for _ in 0..40 {
            v.step(false);
        }
        assert!((v.scale - v.min_scale()).abs() < 1e-6);
    }

    #[test]
    fn steps_walk_the_ladder() {
        let mut v = Viewport::new(Extent::new(1200.0, 800.0), Extent::new(400.0, 300.0), MARGIN);
        v.step(true);
        assert_eq!(v.percent(), 125);
        v.step(false);
        v.step(false);
        assert_eq!(v.percent(), 75);
    }

    #[test]
    fn pan_cannot_lose_the_image() {
        let mut v = tall();
        v.zoom_to(1.0);
        v.pan(-1.0e6, -1.0e6);
        assert_eq!(v.x, 1200.0 - MARGIN - 1440.0, "the right edge stops inside the stage");
        assert_eq!(v.y, 800.0 - MARGIN - 6000.0, "the bottom stops inside the stage");
        v.pan(1.0e6, 1.0e6);
        assert_eq!((v.x, v.y), (MARGIN, MARGIN));
        // An axis the image fits on stays centred.
        let mut small = Viewport::new(Extent::new(1200.0, 800.0), Extent::new(100.0, 50.0), MARGIN);
        small.pan(500.0, 500.0);
        assert_eq!((small.x, small.y), (550.0, 375.0));
        v.pan(f32::NAN, 3.0);
        assert_eq!((v.x, v.y), (MARGIN, MARGIN));
    }

    #[test]
    fn room_for_a_caption_is_kept_under_the_image() {
        let v = Viewport::new(Extent::new(1200.0, 800.0), Extent::new(1440.0, 6000.0), 60.0);
        assert!((v.y + v.drawn().h - (800.0 - 60.0)).abs() < 0.01, "the fitted page ends above the caption");
        let mut v = v;
        v.zoom_to(1.0);
        v.pan(0.0, -1.0e6);
        assert_eq!(v.y + v.drawn().h, 800.0 - 60.0, "scrolled to its end, it stops above the caption");
    }

    #[test]
    fn double_click_toggles_fit_and_actual() {
        let mut v = tall();
        v.toggle_actual(600.0, 400.0);
        assert_eq!(v.percent(), 100);
        v.toggle_actual(600.0, 400.0);
        assert_eq!(v.mode, Mode::Fit);
        assert!(v.percent() < 100);
    }

    #[test]
    fn resize_refits_or_keeps_the_zoom() {
        let mut v = tall();
        v.resize(Extent::new(1600.0, 1000.0));
        assert!((v.drawn().h - (1000.0 - 2.0 * MARGIN)).abs() < 0.01);
        v.zoom_to(2.0);
        v.resize(Extent::new(900.0, 600.0));
        assert_eq!(v.scale, 2.0);
        assert!(v.x <= MARGIN && v.x >= 900.0 - MARGIN - v.drawn().w);
    }

    #[test]
    fn bad_sizes_do_not_poison_the_view() {
        let v = Viewport::new(Extent::new(0.0, f32::NAN), Extent::new(0.0, 0.0), f32::NAN);
        assert!(v.scale.is_finite() && v.x.is_finite() && v.y.is_finite());
    }
}
