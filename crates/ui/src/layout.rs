//! Layout helpers that fix this GPUI version's traps once, so views do not rediscover them.
//!
//! - `div()` is a flex *row* by default and a row does not shrink below its content.
//!   [`row`] and [`col`] say what they are and allow children to shrink (`min-width: 0`).
//! - Text in a shrink-to-fit container is measured at one-line width and never ellipsizes.
//!   [`truncate`] lays the text out in an absolutely positioned box inside a definite-width
//!   cell, which is the only arrangement that ends a long label in `…`.
//! - Children of a column inside a scroll container shrink unless they are `flex_none`.

use gpui::{Div, SharedString, Styled, div, prelude::*, px};

use crate::theme::{BODY, LINE};

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
/// The cell is `BODY` line height (20px); chain `.h(px(..))` and a text style for another size.
/// Colour, family and weight are inherited from the parent.
pub fn truncate(text: impl Into<SharedString>) -> Div {
    truncate_lh(text, BODY.line)
}

/// [`truncate`] for a line height other than 20px, e.g. 18 for labels and 16 for captions.
pub fn truncate_lh(text: impl Into<SharedString>, line_height: f32) -> Div {
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(line_height))
        .child(div().absolute().inset_0().truncate().child(text.into()))
}

/// Like [`truncate`] but for a column (or any block): full width, fixed height, no flex growth.
pub fn truncate_block(text: impl Into<SharedString>, line_height: f32) -> Div {
    div()
        .relative()
        .w_full()
        .flex_none()
        .h(px(line_height))
        .child(div().absolute().inset_0().truncate().child(text.into()))
}
