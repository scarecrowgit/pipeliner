//! Visual identity of an entry (`looks` / `kindOf` in the mockup) and small shared pieces.

use gpui::{Div, FontWeight, Hsla, SharedString, div, prelude::*, px};
use ui::icon;
use ui::theme::*;

use crate::data::{Entry, Kind};

/// Icon + tile colors + name/kind colors. Agents created by another agent use the spawn palette.
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
        Kind::Orchestrator => ("router", ACCENT_SOFT, ACCENT_TEXT),
        Kind::Pipeline => ("pipelines", INFO_SOFT, INFO_TEXT),
        Kind::Agent => ("agent", SURFACE_HOVER, TEXT_SECONDARY),
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
    if e.created_by.is_some() {
        Looks {
            icon: "spawn",
            tile_bg: SPAWN_SOFT,
            tile_fg: SPAWN_TEXT,
            name_color: SPAWN_TEXT,
            kind_color: SPAWN_TEXT,
        }
    } else {
        kind_looks(e.kind)
    }
}

/// Rounded square with a centered icon (28/36px heads, 22px member rows).
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

/// 22px member tile (team rows, add list).
pub fn small_tile(l: Looks) -> Div {
    tile(l.icon, 22., RADIUS_MD, 12., l.tile_bg, l.tile_fg)
}

/// "Name <muted kind>" inline pair used in team rows and the add list.
pub fn name_with_kind(name: &str, kind: &str) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(4.))
        .min_w_0()
        .child(div().truncate().child(SharedString::from(name.to_string())))
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(SharedString::from(kind.to_string())),
        )
}

/// Bold section heading inside the detail panel (`font-weight: 500`).
pub fn heading(text: &'static str) -> Div {
    div().font_weight(FontWeight::MEDIUM).child(text)
}
