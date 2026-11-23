//! The painted dot grid behind the nodes.

use std::ops::Range;

use gpui::{App, Bounds, IntoElement, RenderOnce, Window, canvas, fill, point, prelude::*, px, size};
use ui::theme::LINE;

use super::geometry::GRID_UNIT;
use super::viewport::Viewport;

/// Diameter of a dot, px.
const DOT_SIZE: f32 = 2.0;
/// Dots closer than this many pixels are not drawn: they would be a haze, not a grid.
pub const MIN_DOT_SPACING: f32 = 6.0;
/// The most dots one frame paints.
const MAX_DOTS: i64 = 60_000;

/// The distance in pixels between two dots at `zoom`, or `None` when they would be denser
/// than [`MIN_DOT_SPACING`].
pub fn dot_spacing(zoom: f32) -> Option<f32> {
    let step = GRID_UNIT * zoom;
    (step.is_finite() && step >= MIN_DOT_SPACING).then_some(step)
}

/// The indices `k` of the dots `offset + k × step` that fall in `from..=to` (all in pixels
/// along one axis). Empty for a bad step or non-finite input.
pub fn dot_range(offset: f32, step: f32, from: f32, to: f32) -> Range<i64> {
    if !(offset.is_finite() && step.is_finite() && from.is_finite() && to.is_finite()) || step <= 0.0 {
        return 0..0;
    }
    let first = ((from - offset) / step).ceil() as i64;
    let last = ((to - offset) / step).floor() as i64;
    if last < first { 0..0 } else { first..last + 1 }
}

/// The dots of the canvas background: [`LINE`] coloured, one every 20 canvas units, scaled
/// with the zoom and moving with the pan. Hidden when they would be denser than a few pixels.
///
/// Fills its parent: put it first in a `relative` container that clips (`overflow_hidden`).
#[derive(IntoElement)]
pub struct DotGrid {
    viewport: Viewport,
}

impl DotGrid {
    /// The grid for `viewport`.
    pub fn new(viewport: Viewport) -> Self {
        Self { viewport }
    }
}

impl RenderOnce for DotGrid {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let vp = self.viewport.sanitized();
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let Some(step) = dot_spacing(vp.zoom) else { return };
                let visible: Bounds<gpui::Pixels> = bounds.intersect(&window.content_mask().bounds);
                let (w, h) = (f32::from(visible.size.width), f32::from(visible.size.height));
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                // Work in pixels from the canvas element's top-left.
                let (left, top) = (
                    f32::from(visible.origin.x - bounds.origin.x),
                    f32::from(visible.origin.y - bounds.origin.y),
                );
                let cols = dot_range(vp.origin.x, step, left, left + w);
                let rows = dot_range(vp.origin.y, step, top, top + h);
                if (cols.end - cols.start) * (rows.end - rows.start) > MAX_DOTS {
                    return;
                }
                for row in rows {
                    for col in cols.clone() {
                        let x = vp.origin.x + col as f32 * step;
                        let y = vp.origin.y + row as f32 * step;
                        let at = point(
                            bounds.origin.x + px(x - DOT_SIZE / 2.0),
                            bounds.origin.y + px(y - DOT_SIZE / 2.0),
                        );
                        window.paint_quad(
                            fill(Bounds::new(at, size(px(DOT_SIZE), px(DOT_SIZE))), LINE)
                                .corner_radii(px(DOT_SIZE / 2.0)),
                        );
                    }
                }
            },
        )
        .absolute()
        .size_full()
    }
}
