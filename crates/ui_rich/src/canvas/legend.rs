//! The legend at the foot of the canvas: routes, spawns, selected path.

use gpui::{App, IntoElement, ParentElement, RenderOnce, Window, div, prelude::*, px};
use ui::theme::*;

use super::paint::dashed_rule;

/// Length of a line swatch, px.
const SWATCH_WIDTH: f32 = 16.0;
/// Thickness of a line swatch (the edges' 1.5 stroke).
const SWATCH_THICKNESS: f32 = 1.5;
/// Space between the items, and between a swatch and its word.
const ITEM_GAP: f32 = 14.0;

/// What a swatch looks like.
enum Swatch {
    Solid(gpui::Hsla),
    Dashed(gpui::Hsla),
}

/// The three-item legend of the pipeline canvas: `routes` (grey line), `spawns` (dashed
/// violet line, violet word) and `selected path` (accent line). Label type, muted.
///
/// Position it yourself, e.g. `absolute().left(px(16.)).bottom(px(12.))`.
#[derive(IntoElement, Default)]
pub struct Legend;

impl Legend {
    /// The canvas legend.
    pub fn new() -> Self {
        Self
    }
}

fn item(swatch: Swatch, word: &'static str, word_color: gpui::Hsla) -> impl IntoElement {
    let line = match swatch {
        Swatch::Solid(c) => div().w(px(SWATCH_WIDTH)).h(px(SWATCH_THICKNESS)).bg(c).into_any_element(),
        Swatch::Dashed(c) => div()
            .w(px(SWATCH_WIDTH))
            .h(px(SWATCH_THICKNESS))
            .child(dashed_rule(c, 4.0, 3.0, SWATCH_THICKNESS))
            .into_any_element(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SPACE_1_5))
        .child(line)
        .child(div().text_color(word_color).child(word))
}

impl RenderOnce for Legend {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(ITEM_GAP))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(item(Swatch::Solid(LINE_STRONG), "routes", TEXT_MUTED))
            .child(item(Swatch::Dashed(SPAWN_LINE), "spawns", SPAWN_TEXT))
            .child(item(Swatch::Solid(ACCENT), "selected path", TEXT_MUTED))
    }
}
