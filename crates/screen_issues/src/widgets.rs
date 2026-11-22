//! Small building blocks shared by the board, list and detail views.

use gpui::{FontWeight, Hsla, SharedString, div, prelude::*, px};
use ui::icon;
use ui::theme::*;

use crate::data::RunnerKind;

/// Initials avatar with the mockup's exact sizes (22px / 10px on cards, 20px / 9px in the panel).
pub fn person_avatar(initials: &str, size: f32, font: f32) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(SURFACE_HOVER)
        .border_1()
        .border_color(LINE_STRONG)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(font))
        .line_height(px(font + 2.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(TEXT_SECONDARY)
        .child(SharedString::from(initials.to_string()))
}

/// 22px rounded tile with the runner-kind icon (12px).
pub fn runner_tile(kind: RunnerKind) -> gpui::Div {
    let (bg, fg) = kind.tile_colors();
    div()
        .size(px(22.))
        .flex_none()
        .rounded(px(RADIUS_MD))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(kind.icon(), 12., fg))
}

/// 12px text in a given color.
pub fn small(text: impl Into<SharedString>, color: Hsla) -> gpui::Div {
    div()
        .text_size(px(12.))
        .text_color(color)
        .child(text.into())
}

/// Spawn-colored "by Analyst" pill (22px).
pub fn spawn_pill(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(22.))
        .px(px(8.))
        .rounded_full()
        .bg(SPAWN_SOFT)
        .text_color(SPAWN_TEXT)
        .text_size(px(12.))
        .whitespace_nowrap()
        .child(icon("spawn", 12., SPAWN_TEXT))
        .child(text.into())
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
