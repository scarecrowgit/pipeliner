//! Layout helpers that fix this GPUI version's traps once, so views do not rediscover them.
//!
//! - `div()` is a flex *row* by default and a row does not shrink below its content.
//!   [`row`] and [`col`] say what they are and allow children to shrink (`min-width: 0`).
//! - `nowrap` text is measured once per frame, often at min-content width, and then shows only
//!   its first letters. [`truncate`] keeps a label to one line with `line_clamp(1)` instead,
//!   which GPUI re-measures at the final width, so it ends in `…` only when it does not fit.
//! - Children of a column inside a scroll container shrink unless they are `flex_none`.

use gpui::{Div, SharedString, Styled, div, prelude::*, px};

use crate::theme::{BODY, LINE};

pub use crate::truncate::{
    clamp_styled, clean_highlights, middle_cut, truncate_middle, truncate_styled, truncate_styled_block,
};

/// A horizontal flex row with vertically centred children that may shrink.
///
/// Unlike the CSS `.row` it has no gap; set `.gap(px(SPACE_2))` where one is wanted.
pub fn row() -> Div {
    div().flex().flex_row().items_center().min_w_0()
}

/// A vertical flex column whose children stretch and may shrink.
pub fn col() -> Div {
    div().flex().flex_col().min_w_0()
}

/// A flexible filler: takes the remaining space of its row and may shrink below its content.
pub fn grow() -> Div {
    div().flex_1().min_w_0()
}

/// A horizontal 1px `LINE` rule across the parent.
pub fn hr() -> Div {
    div().h(px(1.)).w_full().flex_none().bg(LINE)
}

/// A vertical 1px `LINE` rule down the parent.
pub fn vr() -> Div {
    div().w(px(1.)).h_full().flex_none().bg(LINE)
}

/// Single-line text that fills the rest of its flex row and ends in `…` when it does not fit.
///
/// In a shrink-to-fit parent (a select trigger with no width) the cell is as wide as the
/// text; in a narrower row it shrinks and the text ends in `…`.
///
/// The cell is `BODY` line height (20px); chain `.h(px(..))` and a text style for another size.
/// Colour, family and weight are inherited from the parent.
pub fn truncate(text: impl Into<SharedString>) -> Div {
    truncate_lh(text, BODY.line)
}

/// One line of `text` that ends in `…` at whatever width its box is given.
///
/// GPUI 0.2.2 measures `nowrap` text once per frame and keeps that first size, which is
/// often taken at min-content width (a few letters). Wrapping text is re-measured whenever
/// the width changes, so the line is kept to one by `line_clamp(1)` instead of `nowrap`.
fn one_line(text: SharedString) -> Div {
    div().w_full().min_w_0().overflow_hidden().whitespace_normal().line_clamp(1).text_ellipsis().child(text)
}

/// [`truncate`] for a line height other than 20px, e.g. 18 for labels and 16 for captions.
pub fn truncate_lh(text: impl Into<SharedString>, line_height: f32) -> Div {
    div()
        .flex()
        .flex_grow()
        .flex_shrink()
        .min_w_0()
        .h(px(line_height))
        .overflow_hidden()
        .child(one_line(text.into()))
}

/// Like [`truncate`] but for a column (or any block): full width, fixed height, no flex growth.
pub fn truncate_block(text: impl Into<SharedString>, line_height: f32) -> Div {
    div()
        .flex()
        .w_full()
        .min_w_0()
        .flex_none()
        .h(px(line_height))
        .overflow_hidden()
        .child(one_line(text.into()))
}
