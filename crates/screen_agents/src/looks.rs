//! Visual identity of an entry (icon, tile and name colours) and small shared pieces.

use gpui::{Div, FontWeight, Hsla, SharedString, div, prelude::*, px};
use ui::icon;
use ui::theme::*;

use crate::model::{Entry, Kind, StepRole};

/// Icon + tile colors + name/kind colors. Tasks created by an agent use the spawn palette.
#[derive(Clone, Copy)]
pub struct Looks {
    pub icon: &'static str,
    pub tile_bg: Hsla,
    pub tile_fg: Hsla,
    pub name_color: Hsla,
    pub kind_color: Hsla,
}

pub fn kind_looks(kind: Kind) -> Looks {
    let (icon, tile_bg, tile_fg) = match kind {
        Kind::Pipeline(_) => ("pipelines", INFO_SOFT, INFO_TEXT),
        Kind::Step(_, StepRole::Lead) => ("router", ACCENT_SOFT, ACCENT_TEXT),
        Kind::Step(_, StepRole::Decision) => ("filter", SURFACE_HOVER, TEXT_SECONDARY),
        Kind::Step(_, StepRole::Agent) | Kind::Delegate(_) => {
            ("agent", SURFACE_HOVER, TEXT_SECONDARY)
        }
        Kind::Chat(_) | Kind::Custom(_) => ("agent", SURFACE_HOVER, TEXT_SECONDARY),
        Kind::Task(_) => {
            return Looks {
                icon: "spawn",
                tile_bg: SPAWN_SOFT,
                tile_fg: SPAWN_TEXT,
                name_color: SPAWN_TEXT,
                kind_color: SPAWN_TEXT,
            };
        }
    };
    Looks {
        icon,
        tile_bg,
        tile_fg,
        name_color: TEXT,
        kind_color: TEXT_MUTED,
    }
}

pub fn looks(e: &Entry) -> Looks {
    kind_looks(e.kind)
}

/// Rounded square with a centered icon (28/36px heads, 22px rows).
pub fn tile(icon_name: &str, size: f32, radius: f32, icon_size: f32, bg: Hsla, fg: Hsla) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(radius))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(icon_name, icon_size, fg))
}

/// Bold section heading inside the detail panel (`font-weight: 500`).
pub fn heading(text: impl Into<SharedString>) -> Div {
    div().font_weight(FontWeight::MEDIUM).child(text.into())
}

/// Small outlined chip (cards, tools).
pub fn chip(text: impl Into<SharedString>, mono: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px(px(6.))
        .rounded(px(RADIUS_SM))
        .bg(SURFACE_RAISED)
        .border_1()
        .border_color(LINE_STRONG)
        .text_size(px(11.))
        .text_color(TEXT_SECONDARY)
        .when(mono, |d| d.font_family(FONT_MONO))
        .child(text.into())
}

/// One 16px-high line that ellipsizes instead of wrapping or hard-clipping: gpui measures
/// nowrap text once, so the text sits in an absolutely positioned box over a sized cell.
pub fn ellipsis_line(text: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .relative()
        .w_full()
        .min_w_0()
        .flex_none()
        .h(px(16.))
        .child(
            div()
                .absolute()
                .inset_0()
                .overflow_hidden()
                // `line_clamp(1)` instead of `whitespace_nowrap()`: gpui caches the first
                // (unconstrained) measurement of nowrap text, so it would never truncate.
                .line_clamp(1)
                .text_ellipsis()
                .text_size(px(size))
                .line_height(px(16.))
                .text_color(color)
                .child(text.into()),
        )
}
