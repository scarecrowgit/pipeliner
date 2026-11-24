//! Graphite tokens (dark), one constant per CSS custom property of `tokens.css`, plus the few
//! raw values `lib.py` uses that the token file does not name.
//!
//! This is the only module that may contain hex colours and raw font sizes. Sizes are plain
//! `f32` pixel values; wrap with `px()` where they are used.

use std::sync::Arc;
use std::time::Duration;

use gpui::{BoxShadow, FontFeatures, FontWeight, Hsla, Styled, TextStyleRefinement, point, px};

/// `0xRRGGBB` to an opaque [`Hsla`]. `const`, so tokens can be consts.
pub const fn hex(c: u32) -> Hsla {
    let r = ((c >> 16) & 0xff) as f32 / 255.0;
    let g = ((c >> 8) & 0xff) as f32 / 255.0;
    let b = (c & 0xff) as f32 / 255.0;
    let max = if r > g {
        if r > b { r } else { b }
    } else if g > b {
        g
    } else {
        b
    };
    let min = if r < g {
        if r < b { r } else { b }
    } else if g < b {
        g
    } else {
        b
    };
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

/// The same colour with another alpha.
pub const fn with_alpha(color: Hsla, a: f32) -> Hsla {
    Hsla { a, ..color }
}

// Grounds, darkest to lightest.
/// Page background behind main content.
pub const BG: Hsla = hex(0x08090a);
/// Sidebar, sticky footers, action bars and code blocks.
pub const BG_SIDEBAR: Hsla = hex(0x0b0c0d);
/// Cards, panels, tables. Always with a line border, never a shadow.
pub const SURFACE: Hsla = hex(0x0f1011);
/// Inputs, secondary buttons, search field, hovered table rows.
pub const SURFACE_RAISED: Hsla = hex(0x141518);
/// Active nav item, selected list row, segmented-control thumb.
pub const SURFACE_HOVER: Hsla = hex(0x1c1d21);
/// Fully transparent, the rest colour of a hover fade.
pub const TRANSPARENT: Hsla = Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.0 };

// Lines.
/// Row dividers inside a card or timeline.
pub const LINE_SUBTLE: Hsla = hex(0x16171a);
/// Default hairline: card borders, section dividers, header bottoms.
pub const LINE: Hsla = hex(0x1c1d21);
/// Control borders (buttons, inputs, chips, kbd).
pub const LINE_STRONG: Hsla = hex(0x2e3035);

// Text.
/// Primary text and numbers.
pub const TEXT: Hsla = hex(0xf7f8f8);
/// Body copy in descriptions, inactive nav labels.
pub const TEXT_SECONDARY: Hsla = hex(0xd0d6e0);
/// Labels, meta, timestamps, table headers. The lowest grey allowed for text.
pub const TEXT_MUTED: Hsla = hex(0x8a8f98);
/// Disabled controls and decorative separators only.
pub const TEXT_DISABLED: Hsla = hex(0x62666d);

// Accent.
/// Focus ring, active tab underline, progress bars, selected-row edge.
pub const ACCENT: Hsla = hex(0x7170ff);
/// Primary button and count badge fill.
pub const ACCENT_STRONG: Hsla = hex(0x5e6ad2);
/// Primary button hover.
pub const ACCENT_STRONG_HOVER: Hsla = hex(0x5561c9);
/// Links and accent-coloured labels.
pub const ACCENT_TEXT: Hsla = hex(0xa5a4ff);
/// Draft and edited pills, accent icon tiles.
pub const ACCENT_SOFT: Hsla = hex(0x1e1e3f);
/// Border of a selected card or an edited rule.
pub const ACCENT_RING: Hsla = hex(0x3a3a7a);
/// 3px outer halo around a selected card; also the focused input halo.
pub const ACCENT_GLOW: Hsla = hex(0x15152b);
/// Text and icons on `ACCENT_STRONG`.
pub const ON_ACCENT: Hsla = hex(0xffffff);
/// Border of a kbd hint inside a primary button (`rgba(255,255,255,.35)` in `lib.py`).
pub const ON_ACCENT_LINE: Hsla = with_alpha(ON_ACCENT, 0.35);

// Status.
/// Live, done, tests passed, diff additions count.
pub const SUCCESS: Hsla = hex(0x4cb782);
/// Waiting for a human, high priority.
pub const WARNING: Hsla = hex(0xf2c94c);
/// Fill of a waiting-for-human bar or banner.
pub const WARNING_SOFT: Hsla = hex(0x2a2410);
/// Errors, failed tests, denied spawns, destructive text buttons.
pub const DANGER: Hsla = hex(0xeb5757);
/// Hover of a destructive text button; removed diff lines.
pub const DANGER_SOFT: Hsla = hex(0x2a1416);
/// Running state dot and bar.
pub const INFO: Hsla = hex(0x4ea7fc);
/// Icon tile behind source/data nodes.
pub const INFO_SOFT: Hsla = hex(0x13203a);
/// Icon on `INFO_SOFT`.
pub const INFO_TEXT: Hsla = hex(0x6fa8ff);
/// Border of a warning banner (`#4a3e18` in `lib.py`, not a token).
pub const BANNER_WARNING_LINE: Hsla = hex(0x4a3e18);
/// Border of a danger banner (`#4a2226` in `lib.py`, not a token).
pub const BANNER_DANGER_LINE: Hsla = hex(0x4a2226);

// Spawn: nothing else is violet.
/// Dot or marker for an agent created by another agent.
pub const SPAWN: Hsla = hex(0xa78bfa);
/// Timeline bar of a spawned agent.
pub const SPAWN_BAR: Hsla = hex(0x8b7fe8);
/// Text naming a spawned agent or a spawn event.
pub const SPAWN_TEXT: Hsla = hex(0xc4b5fd);
/// Spawn chip and request banner fill.
pub const SPAWN_SOFT: Hsla = hex(0x171329);
/// Border of a spawn request; tree guide lines under a spawning agent.
pub const SPAWN_LINE: Hsla = hex(0x3b3463);

// Diff.
/// Added line in a diff.
pub const DIFF_ADD_BG: Hsla = hex(0x10261c);
/// Text of an added diff line.
pub const DIFF_ADD_TEXT: Hsla = hex(0x8fddb5);
/// Removed line in a diff.
pub const DIFF_DEL_BG: Hsla = hex(0x2a1416);
/// Text of a removed diff line.
pub const DIFF_DEL_TEXT: Hsla = hex(0xf3a6a6);

/// Behind a dialog or the command palette (`rgba(8, 9, 10, 0.72)`).
pub const SCRIM: Hsla = with_alpha(BG, 0.72);

/// Behind a full-window layer such as the image viewer: dark enough that a picture reads on
/// it without the page showing through (`rgba(8, 9, 10, 0.96)`).
pub const SCRIM_STRONG: Hsla = with_alpha(BG, 0.96);

// Spacing.
/// Half of `SPACE_1`: tight stacks (nav list gap is 1, stat label gap 2).
pub const SPACE_0_5: f32 = 2.0;
/// Icon-to-count, chip internals.
pub const SPACE_1: f32 = 4.0;
/// Button label to icon gap.
pub const SPACE_1_5: f32 = 6.0;
/// Gap between buttons; small stacks.
pub const SPACE_2: f32 = 8.0;
/// Menu row gap, nav item gap, panel header margin.
pub const SPACE_2_5: f32 = 10.0;
/// Sidebar padding; gap between cards in a column.
pub const SPACE_3: f32 = 12.0;
/// Card padding, primary button inline padding trim.
pub const SPACE_3_5: f32 = 14.0;
/// Gap between panels; table cell inline padding.
pub const SPACE_4: f32 = 16.0;
/// Gap between tabs; side panel padding.
pub const SPACE_5: f32 = 20.0;
/// Page padding and header inline padding.
pub const SPACE_6: f32 = 24.0;
/// Rare. Separates unrelated groups.
pub const SPACE_8: f32 = 32.0;

// Radii.
/// Kbd, inline code.
pub const RADIUS_SM: f32 = 4.0;
/// Nav items, chips, rule chips, steppers, icon tiles.
pub const RADIUS_MD: f32 = 6.0;
/// Buttons, inputs, selects, banners.
pub const RADIUS_LG: f32 = 8.0;
/// Cards, panels, tables.
pub const RADIUS_XL: f32 = 10.0;
/// Dialogs and the command palette.
pub const RADIUS_DIALOG: f32 = 12.0;
/// Status pills, count badges, dots.
pub const RADIUS_FULL: f32 = 999.0;

// Dialogs.
/// Default dialog width (the command palette's).
pub const DIALOG_WIDTH: f32 = 640.0;
/// Default distance of a dialog from the window's top, as a fraction of its height (12vh).
pub const DIALOG_TOP: f32 = 0.12;
/// Gap between blocks of a dialog body.
pub const DIALOG_GAP: f32 = 18.0;

// Control and row sizes.
/// Buttons and inputs in headers and action bars.
pub const CONTROL_HEIGHT: f32 = 40.0;
/// Icon buttons, compact inputs and selects, steppers, the sidebar search field.
pub const CONTROL_HEIGHT_MD: f32 = 32.0;
/// Filter chips, compact buttons, rule chips.
pub const CONTROL_HEIGHT_SM: f32 = 28.0;
/// Icon button in the collapsed sidebar rail.
pub const RAIL_BUTTON: f32 = 36.0;
/// List rows in the inbox and queues.
pub const ROW_HEIGHT: f32 = 40.0;
/// Sidebar nav items.
pub const NAV_ITEM_HEIGHT: f32 = 32.0;
/// Sidebar list rows (pipelines, chats).
pub const LIST_ITEM_HEIGHT: f32 = 28.0;
/// A child row under a spawning chat.
pub const LIST_ITEM_CHILD_HEIGHT: f32 = 26.0;
/// Popover and menu rows.
pub const MENU_ITEM_HEIGHT: f32 = 36.0;
/// Underline tabs under a page header.
pub const TAB_HEIGHT: f32 = 44.0;
/// Table header row.
pub const TABLE_HEAD_HEIGHT: f32 = 36.0;
/// Table body row.
pub const TABLE_ROW_HEIGHT: f32 = 48.0;
/// Table group header row.
pub const TABLE_GROUP_HEIGHT: f32 = 32.0;
/// Card header row.
pub const CARD_HEAD_HEIGHT: f32 = 44.0;
/// Card footer row.
pub const CARD_FOOT_HEIGHT: f32 = 40.0;
/// Page header.
pub const HEADER_HEIGHT: f32 = 56.0;
/// Action bar at the foot of a page.
pub const ACTION_BAR_HEIGHT: f32 = 64.0;
/// Sidebar width.
pub const SIDEBAR_WIDTH: f32 = 232.0;
/// Collapsed sidebar rail width.
pub const SIDEBAR_RAIL_WIDTH: f32 = 56.0;
/// Right-hand detail panel width.
pub const PANEL_WIDTH: f32 = 360.0;

// Z-order of floating layers (the overlay host orders by these).
/// Popovers, menus, pickers anchored to a trigger.
pub const Z_POPOVER: u16 = 20;
/// The scrim behind a dialog.
pub const Z_OVERLAY: u16 = 40;
/// Dialog and command palette panels.
pub const Z_DIALOG: u16 = 50;
/// Toasts, above everything.
pub const Z_TOAST: u16 = 60;

// Motion.
/// Any popup leaving: popover, dialog, toast.
pub const DURATION_EXIT: Duration = Duration::from_millis(100);
/// Background, border and colour changes on hover, select and focus.
pub const DURATION_STATE: Duration = Duration::from_millis(150);
/// Popover, menu, picker, and a confirm revealed inside an open popup.
pub const DURATION_ENTER: Duration = Duration::from_millis(180);
/// Dialog and command palette panel, and their scrim.
pub const DURATION_OVERLAY: Duration = Duration::from_millis(220);
/// Toast rising from the bottom edge.
pub const DURATION_TOAST: Duration = Duration::from_millis(240);
/// How long a toast stays up when nobody hovers it.
pub const TOAST_STAY: Duration = Duration::from_secs(4);

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier(pub f32, pub f32, pub f32, pub f32);

/// Everything that enters: popovers, dialogs, toasts, reveals.
pub const EASE_OUT: CubicBezier = CubicBezier(0.2, 0.0, 0.0, 1.0);
/// Everything that leaves.
pub const EASE_IN: CubicBezier = CubicBezier(0.4, 0.0, 1.0, 1.0);
/// CSS `ease`: state fades.
pub const EASE_STANDARD: CubicBezier = CubicBezier(0.25, 0.1, 0.25, 1.0);

impl CubicBezier {
    /// Progress of the curve at time `t` (both 0 to 1): solves x(s) = t, returns y(s).
    pub fn at(self, t: f32) -> f32 {
        let CubicBezier(x1, y1, x2, y2) = self;
        let t = t.clamp(0., 1.);
        let bez = |a: f32, b: f32, s: f32| {
            let u = 1. - s;
            3. * u * u * s * a + 3. * u * s * s * b + s * s * s
        };
        let dbez = |a: f32, b: f32, s: f32| {
            let u = 1. - s;
            3. * u * u * a + 6. * u * s * (b - a) + 3. * s * s * (1. - b)
        };
        let mut s = t;
        for _ in 0..8 {
            let x = bez(x1, x2, s) - t;
            if x.abs() < 1e-5 {
                return bez(y1, y2, s);
            }
            let d = dbez(x1, x2, s);
            if d.abs() < 1e-6 {
                break;
            }
            s = (s - x / d).clamp(0., 1.);
        }
        let (mut lo, mut hi) = (0f32, 1f32);
        s = t;
        for _ in 0..30 {
            let x = bez(x1, x2, s);
            if (x - t).abs() < 1e-5 {
                break;
            }
            if x < t {
                lo = s;
            } else {
                hi = s;
            }
            s = (lo + hi) / 2.;
        }
        bez(y1, y2, s)
    }
}

// Type.
/// Interface font.
pub const FONT_SANS: &str = "Inter";
/// Machine text font.
pub const FONT_MONO: &str = "JetBrains Mono";

/// One entry of the type scale: size, line height, weight and family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    /// Font size in px.
    pub size: f32,
    /// Line height in px.
    pub line: f32,
    /// Font weight.
    pub weight: FontWeight,
    /// JetBrains Mono instead of Inter.
    pub mono: bool,
}

const fn sans(size: f32, line: f32, weight: f32) -> TypeStyle {
    TypeStyle { size, line, weight: FontWeight(weight), mono: false }
}

const fn mono(size: f32, line: f32) -> TypeStyle {
    TypeStyle { size, line, weight: FontWeight::NORMAL, mono: true }
}

/// Item title on a detail view. One per page. 22/28, 600.
pub const TITLE_LG: TypeStyle = sans(22., 28., 600.);
/// Section title inside an editor. 20/28, 600.
pub const TITLE: TypeStyle = sans(20., 28., 600.);
/// The one number a card is about. 28/32, 600.
pub const STAT: TypeStyle = sans(28., 32., 600.);
/// Numbers in a stats strip. 18/24, 500. Pair with `.tabular()`.
pub const STAT_SM: TypeStyle = sans(18., 24., 500.);
/// Dialog titles (`.dlg-h h2`). 15/22, 500.
pub const TITLE_SM: TypeStyle = sans(15., 22., 500.);
/// Panel and page headers. 14/20, 500.
pub const HEADING: TypeStyle = sans(14., 20., 500.);
/// Default UI text. 13/20, 400.
pub const BODY: TypeStyle = sans(13., 20., 400.);
/// Names of agents, cards and table primaries. 13/20, 500.
pub const BODY_STRONG: TypeStyle = sans(13., 20., 500.);
/// Labels, meta, chips, table headers. 12/18, 400.
pub const LABEL: TypeStyle = sans(12., 18., 400.);
/// Small buttons, selected segments: 12/18, 500.
pub const LABEL_STRONG: TypeStyle = sans(12., 18., 500.);
/// Badges and tags: 12 on a 16 line, 400.
pub const LABEL_TIGHT: TypeStyle = sans(12., 16., 400.);
/// Small captions and sub-labels. 11/16, 400.
pub const CAPTION: TypeStyle = sans(11., 16., 400.);
/// Count badge numbers. 11/16, 600.
pub const CAPTION_STRONG: TypeStyle = sans(11., 16., 600.);
/// Count on a rail button. 10/16, 600.
pub const MICRO: TypeStyle = sans(10., 16., 600.);
/// Initials in a 20px avatar. 9/9, 600.
pub const AVATAR: TypeStyle = sans(9., 9., 600.);
/// Initials in a 24px avatar. 10/10, 600.
pub const AVATAR_LG: TypeStyle = sans(10., 10., 600.);
/// Issue keys, conditions, tool names, diffs, costs in logs. 12/20.
pub const MONO: TypeStyle = mono(12., 20.);
/// Timeline axes and kbd hints only. 11/16.
pub const MONO_SM: TypeStyle = mono(11., 16.);

/// Applies a [`TypeStyle`] to any styled element: `div().type_style(BODY)`.
pub trait TextStyleExt: Styled + Sized {
    /// Sets family, size, line height and weight from the type scale.
    fn type_style(self, t: TypeStyle) -> Self {
        self.font_family(if t.mono { FONT_MONO } else { FONT_SANS })
            .text_size(px(t.size))
            .line_height(px(t.line))
            .font_weight(t.weight)
    }

    /// Tabular figures, for numbers that line up in columns.
    fn tabular(mut self) -> Self {
        let style = Styled::text_style(&mut self).get_or_insert_with(TextStyleRefinement::default);
        style.font_features = Some(sans_features(true));
        self
    }
}

impl<T: Styled> TextStyleExt for T {}

/// Inter's stylistic sets from the mockups' `font-feature-settings` (`cv11`, `ss03`), with
/// tabular figures when `tabular`.
pub fn sans_features(tabular: bool) -> FontFeatures {
    let mut v = vec![("cv11".to_string(), 1), ("ss03".to_string(), 1)];
    if tabular {
        v.push(("tnum".to_string(), 1));
    }
    FontFeatures(Arc::new(v))
}

/// Keyboard focus: a bg-coloured gap, then a 2px accent ring (`--focus-ring`).
pub fn focus_ring() -> Vec<BoxShadow> {
    // Painted in order: the wide accent ring first, the bg gap over its inner half.
    vec![ring(ACCENT, 4.), ring(BG, 2.)]
}

/// Outer halo around a selected card, together with an `ACCENT_RING` border.
pub fn selected_glow() -> Vec<BoxShadow> {
    vec![ring(ACCENT_GLOW, 3.)]
}

/// Halo around a focused input.
pub fn input_glow() -> Vec<BoxShadow> {
    selected_glow()
}

/// Halo around an input that confirms something destructive (type-to-delete).
pub fn danger_glow() -> Vec<BoxShadow> {
    vec![ring(DANGER_SOFT, 3.)]
}

fn ring(color: Hsla, spread: f32) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        // GPUI's shadow shader integrates the blur over ±3 × blur_radius: at 0 it draws
        // nothing. A tenth of a pixel is still a crisp edge.
        blur_radius: px(0.1),
        spread_radius: px(spread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_matches_the_token_file() {
        let c = BG_SIDEBAR.to_rgb();
        assert!((c.r * 255. - 11.).abs() < 0.6);
        assert!((c.g * 255. - 12.).abs() < 0.6);
        assert!((c.b * 255. - 13.).abs() < 0.6);
        assert_eq!(TEXT.a, 1.);
        assert_eq!(SCRIM.a, 0.72);
    }

    #[test]
    fn type_scale_follows_the_tokens() {
        assert_eq!((BODY.size, BODY.line, BODY.weight), (13., 20., FontWeight::NORMAL));
        assert_eq!((TITLE_LG.size, TITLE_LG.line), (22., 28.));
        assert_eq!(STAT_SM.weight, FontWeight::MEDIUM);
        assert!([MONO, MONO_SM].iter().all(|t| t.mono) && [LABEL, BODY].iter().all(|t| !t.mono));
    }

    #[test]
    fn easing_curves_start_and_end_on_the_axes() {
        for e in [EASE_OUT, EASE_IN, EASE_STANDARD] {
            assert!(e.at(0.).abs() < 1e-3);
            assert!((e.at(1.) - 1.).abs() < 1e-3);
        }
    }
}
