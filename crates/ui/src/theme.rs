//! Graphite design tokens (dark), 1:1 with the mockup's CSS custom properties.
//!
//! Colors are `Hsla` consts built from hex via [`hex`]. Spacing/radius/sizes are
//! plain `f32` pixel values — wrap with `px()` at the call site.

use gpui::{BoxShadow, Hsla, Rgba, point, px};

/// `0xRRGGBB` -> Hsla (opaque). `const` so tokens can be consts.
pub const fn hex(c: u32) -> Hsla {
    // const-friendly rgb -> hsl
    let r = ((c >> 16) & 0xff) as f32 / 255.0;
    let g = ((c >> 8) & 0xff) as f32 / 255.0;
    let b = (c & 0xff) as f32 / 255.0;
    let max = if r > g { if r > b { r } else { b } } else if g > b { g } else { b };
    let min = if r < g { if r < b { r } else { b } } else if g < b { g } else { b };
    let l = (max + min) / 2.0;
    if max == min {
        return Hsla { h: 0.0, s: 0.0, l, a: 1.0 };
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let mut h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h /= 6.0;
    Hsla { h, s, l, a: 1.0 }
}

/// Same color, different alpha.
pub fn alpha(color: Hsla, a: f32) -> Hsla {
    Hsla { a, ..color }
}

/// Exact sRGB from hex (for places where HSL rounding matters, e.g. canvas paths).
pub fn rgba(c: u32) -> Rgba {
    gpui::rgb(c)
}

// ── Surfaces ────────────────────────────────────────────────────────────
pub const BG: Hsla = hex(0x08090a);
pub const BG_SIDEBAR: Hsla = hex(0x0b0c0d);
pub const SURFACE: Hsla = hex(0x0f1011);
pub const SURFACE_RAISED: Hsla = hex(0x141518);
pub const SURFACE_HOVER: Hsla = hex(0x1c1d21);
pub const LINE_SUBTLE: Hsla = hex(0x16171a);
pub const LINE: Hsla = hex(0x1c1d21);
pub const LINE_STRONG: Hsla = hex(0x2e3035);
/// Canvas dot-grid color (`radial-gradient(circle, #1f2024 1px ...)`).
pub const GRID_DOT: Hsla = hex(0x1f2024);

// ── Text ────────────────────────────────────────────────────────────────
pub const TEXT: Hsla = hex(0xf7f8f8);
pub const TEXT_SECONDARY: Hsla = hex(0xd0d6e0);
pub const TEXT_MUTED: Hsla = hex(0x8a8f98);
pub const TEXT_DISABLED: Hsla = hex(0x62666d);

// ── Accent ──────────────────────────────────────────────────────────────
pub const ACCENT: Hsla = hex(0x7170ff);
pub const ACCENT_STRONG: Hsla = hex(0x5e6ad2);
pub const ACCENT_STRONG_HOVER: Hsla = hex(0x5561c9);
pub const ACCENT_TEXT: Hsla = hex(0xa5a4ff);
pub const ACCENT_SOFT: Hsla = hex(0x1e1e3f);
pub const ACCENT_RING: Hsla = hex(0x3a3a7a);
pub const ACCENT_GLOW: Hsla = hex(0x15152b);
pub const ON_ACCENT: Hsla = hex(0xffffff);

// ── Status ──────────────────────────────────────────────────────────────
pub const SUCCESS: Hsla = hex(0x4cb782);
pub const WARNING: Hsla = hex(0xf2c94c);
pub const WARNING_SOFT: Hsla = hex(0x2a2410);
pub const DANGER: Hsla = hex(0xeb5757);
pub const DANGER_SOFT: Hsla = hex(0x2a1416);
pub const INFO: Hsla = hex(0x4ea7fc);
pub const INFO_SOFT: Hsla = hex(0x13203a);
pub const INFO_TEXT: Hsla = hex(0x6fa8ff);

// ── Spawn (sub-agents) ──────────────────────────────────────────────────
pub const SPAWN: Hsla = hex(0xa78bfa);
pub const SPAWN_BAR: Hsla = hex(0x8b7fe8);
pub const SPAWN_TEXT: Hsla = hex(0xc4b5fd);
pub const SPAWN_SOFT: Hsla = hex(0x171329);
pub const SPAWN_LINE: Hsla = hex(0x3b3463);

// ── Diff ────────────────────────────────────────────────────────────────
pub const DIFF_ADD_BG: Hsla = hex(0x10261c);
pub const DIFF_ADD_TEXT: Hsla = hex(0x8fddb5);
pub const DIFF_DEL_BG: Hsla = hex(0x2a1416);
pub const DIFF_DEL_TEXT: Hsla = hex(0xf3a6a6);

// ── Spacing (px) ────────────────────────────────────────────────────────
pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const SPACE_4: f32 = 16.0;
pub const SPACE_5: f32 = 20.0;
pub const SPACE_6: f32 = 24.0;
pub const SPACE_8: f32 = 32.0;

// ── Radius (px) ─────────────────────────────────────────────────────────
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 6.0;
pub const RADIUS_LG: f32 = 8.0;
pub const RADIUS_XL: f32 = 10.0;
pub const RADIUS_FULL: f32 = 999.0;

// ── Sizes (px) ──────────────────────────────────────────────────────────
pub const CONTROL_HEIGHT: f32 = 40.0;
pub const CONTROL_HEIGHT_SM: f32 = 28.0;
pub const ROW_HEIGHT: f32 = 40.0;
pub const NAV_ITEM_HEIGHT: f32 = 32.0;
pub const TAB_HEIGHT: f32 = 44.0;
pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const SIDEBAR_COMPACT_WIDTH: f32 = 56.0;
/// Page header height used by every screen (`min-height: 56px`).
pub const HEADER_HEIGHT: f32 = 56.0;

// ── Type ────────────────────────────────────────────────────────────────
pub const FONT_SANS: &str = "Inter";
pub const FONT_MONO: &str = "JetBrains Mono";
/// Body text: 13px / 20px.
pub const TEXT_BODY: f32 = 13.0;
pub const LINE_BODY: f32 = 20.0;
/// Labels: 12px / 18px.
pub const TEXT_LABEL: f32 = 12.0;
/// Mono: 12px / 20px; mono-sm: 11px / 16px.
pub const TEXT_MONO: f32 = 12.0;
pub const TEXT_MONO_SM: f32 = 11.0;
/// Headings 14px/20px 500; titles 20px/28px 600; title-lg 22px/28px 600.
pub const TEXT_HEADING: f32 = 14.0;
pub const TEXT_TITLE: f32 = 20.0;
pub const TEXT_TITLE_LG: f32 = 22.0;
pub const TEXT_STAT: f32 = 28.0;
pub const TEXT_STAT_SM: f32 = 18.0;

/// `--selected-glow: 0 0 0 3px #15152b` (selected cards / nodes).
pub fn selected_glow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: ACCENT_GLOW,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(3.),
    }]
}

/// Soft drop shadow for popovers / menus.
pub fn popover_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Hsla { h: 0., s: 0., l: 0., a: 0.5 },
        offset: point(px(0.), px(8.)),
        blur_radius: px(24.),
        spread_radius: px(0.),
    }]
}
