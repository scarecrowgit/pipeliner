//! Small painted pieces shared by the canvas and the charts.

use gpui::{Canvas, Hsla, PathBuilder, canvas, point, prelude::*, px};

/// A horizontal dashed rule that fills the width of its parent and is `thickness` px tall.
///
/// CSS `border-top: 1px dashed` without the border: GPUI draws dashes only along a stroked
/// path, so this paints one.
pub(crate) fn dashed_rule(color: Hsla, dash: f32, gap: f32, thickness: f32) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let y = bounds.origin.y + bounds.size.height / 2.0;
            let mut path = PathBuilder::stroke(px(thickness)).dash_array(&[px(dash), px(gap)]);
            path.move_to(point(bounds.origin.x, y));
            path.line_to(point(bounds.origin.x + bounds.size.width, y));
            // An empty or degenerate path fails to build; there is nothing to draw then.
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w_full()
    .h(px(thickness))
}
