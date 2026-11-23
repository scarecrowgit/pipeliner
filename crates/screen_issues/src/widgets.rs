//! Small building blocks shared by the board, list and detail views.

use gpui::{Div, FontWeight, Hsla, SharedString, div, prelude::*, px};
use ui::status_dot;
use ui::theme::*;

use crate::look::Line;

/// 12px text in a given color.
pub fn small(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .text_size(px(12.))
        .text_color(color)
        .child(text.into())
}

/// Machine text (keys, durations): 12px mono.
pub fn mono(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_size(px(TEXT_MONO))
        .text_color(color)
        .child(text.into())
}

/// Section heading inside the detail panel, with optional muted meta on the right.
pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().font_weight(FontWeight::MEDIUM).child(text.into()))
}

/// Status dot + coloured words (the card's run line).
pub fn status_line(line: &Line) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .w_full()
        .min_w_0()
        .text_size(px(12.))
        .line_height(px(16.))
        .child(status_dot(line.dot, true))
        .child(
            div()
                .flex_1()
                .ellipsis()
                .text_color(line.color)
                .child(line.text.clone()),
        )
}

/// Single-line text cut with "…".
///
/// gpui's `truncate()` (nowrap + ellipsis) caches the first text measurement, which
/// can be taken under an intrinsic-size pass with a wider width, so the text is then
/// hard-clipped without the ellipsis. A one-line clamp keeps normal wrapping, whose
/// layout is re-measured whenever the width changes, so the "…" lands at the real edge.
pub trait Ellipsis: Styled + Sized {
    fn ellipsis(self) -> Self {
        self.min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .line_clamp(1)
    }
}

impl<T: Styled> Ellipsis for T {}
