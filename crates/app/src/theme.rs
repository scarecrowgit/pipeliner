//! Design tokens (dark, Linear-like) and text-style helpers.
//!
//! Screens never hardcode colors or sizes: use the constants here
//! (`theme::SURFACE`, `theme::TEXT_MUTED`, `theme::S4`, `theme::RADIUS_LG`...) and the
//! [`TextStyles`] extension (`.text_body()`, `.text_label()`, `.text_mono()`, `.fg_muted()`...).
//!
//! Mirrors `design/screens/*.html` `:root` custom properties one to one.

// Shared API for screens: some tokens / helpers are not used by every build of the screens.
#![allow(dead_code)]

use std::sync::OnceLock;
use std::time::Duration;

use gpui::{px, App, FontWeight, Hsla, Pixels, SharedString, Styled};
use harness_core::Status;

// ---------------------------------------------------------------------------
// Color helpers (const, so tokens are real constants)
// ---------------------------------------------------------------------------

/// `0xRRGGBB` -> [`Hsla`] at compile time.
pub const fn hex(rgb: u32) -> Hsla {
    hexa(rgb, 1.0)
}

/// `0xRRGGBB` + alpha -> [`Hsla`] at compile time.
pub const fn hexa(rgb: u32, a: f32) -> Hsla {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    let max = if r > g { if r > b { r } else { b } } else if g > b { g } else { b };
    let min = if r < g { if r < b { r } else { b } } else if g < b { g } else { b };
    let l = (max + min) / 2.0;
    if max == min {
        return Hsla { h: 0.0, s: 0.0, l, a };
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    Hsla { h: h / 6.0, s, l, a }
}

/// Same color with a different alpha.
pub const fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { h: c.h, s: c.s, l: c.l, a }
}

// ---------------------------------------------------------------------------
// Color tokens
// ---------------------------------------------------------------------------

pub const BG: Hsla = hex(0x08090a);
pub const BG_SIDEBAR: Hsla = hex(0x0b0c0d);
pub const SURFACE: Hsla = hex(0x0f1011);
pub const SURFACE_RAISED: Hsla = hex(0x141518);
pub const SURFACE_HOVER: Hsla = hex(0x1c1d21);

pub const LINE_SUBTLE: Hsla = hex(0x16171a);
pub const LINE: Hsla = hex(0x1c1d21);
pub const LINE_STRONG: Hsla = hex(0x2e3035);

pub const TEXT: Hsla = hex(0xf7f8f8);
pub const TEXT_SECONDARY: Hsla = hex(0xd0d6e0);
pub const TEXT_MUTED: Hsla = hex(0x8a8f98);
pub const TEXT_DISABLED: Hsla = hex(0x62666d);

pub const ACCENT: Hsla = hex(0x7170ff);
pub const ACCENT_STRONG: Hsla = hex(0x5e6ad2);
pub const ACCENT_STRONG_HOVER: Hsla = hex(0x5561c9);
pub const ACCENT_TEXT: Hsla = hex(0xa5a4ff);
pub const ACCENT_SOFT: Hsla = hex(0x1e1e3f);
pub const ACCENT_RING: Hsla = hex(0x3a3a7a);
pub const ACCENT_GLOW: Hsla = hex(0x15152b);
pub const ON_ACCENT: Hsla = hex(0xffffff);

pub const SUCCESS: Hsla = hex(0x4cb782);
pub const SUCCESS_SOFT: Hsla = hex(0x10261c);
pub const WARNING: Hsla = hex(0xf2c94c);
pub const WARNING_SOFT: Hsla = hex(0x2a2410);
pub const DANGER: Hsla = hex(0xeb5757);
pub const DANGER_SOFT: Hsla = hex(0x2a1416);
pub const INFO: Hsla = hex(0x4ea7fc);
pub const INFO_SOFT: Hsla = hex(0x13203a);
pub const INFO_TEXT: Hsla = hex(0x6fa8ff);

pub const SPAWN: Hsla = hex(0xa78bfa);
pub const SPAWN_BAR: Hsla = hex(0x8b7fe8);
pub const SPAWN_TEXT: Hsla = hex(0xc4b5fd);
pub const SPAWN_SOFT: Hsla = hex(0x171329);
pub const SPAWN_LINE: Hsla = hex(0x3b3463);

pub const DIFF_ADD_BG: Hsla = hex(0x10261c);
pub const DIFF_ADD_TEXT: Hsla = hex(0x8fddb5);
pub const DIFF_DEL_BG: Hsla = hex(0x2a1416);
pub const DIFF_DEL_TEXT: Hsla = hex(0xf3a6a6);

/// Modal backdrop.
pub const SCRIM: Hsla = hexa(0x08090a, 0.72);
pub const TRANSPARENT: Hsla = Hsla { h: 0.0, s: 0.0, l: 0.0, a: 0.0 };

// ---------------------------------------------------------------------------
// Spacing, radius, sizes
// ---------------------------------------------------------------------------

pub const S1: Pixels = px(4.);
pub const S2: Pixels = px(8.);
pub const S3: Pixels = px(12.);
pub const S4: Pixels = px(16.);
pub const S5: Pixels = px(20.);
pub const S6: Pixels = px(24.);
pub const S8: Pixels = px(32.);

pub const RADIUS_SM: Pixels = px(4.);
pub const RADIUS_MD: Pixels = px(6.);
pub const RADIUS_LG: Pixels = px(8.);
pub const RADIUS_XL: Pixels = px(10.);
pub const RADIUS_FULL: Pixels = px(999.);

pub const CONTROL_H: Pixels = px(40.);
pub const CONTROL_H_SM: Pixels = px(28.);
pub const ROW_H: Pixels = px(40.);
pub const NAV_ITEM_H: Pixels = px(32.);
pub const TAB_H: Pixels = px(44.);
pub const SIDEBAR_W: Pixels = px(232.);
/// Height of a screen's page header bar.
pub const HEADER_H: Pixels = px(64.);
pub const ICON_SM: Pixels = px(14.);
pub const ICON: Pixels = px(16.);

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

pub const DUR_EXIT: Duration = Duration::from_millis(100);
pub const DUR_STATE: Duration = Duration::from_millis(150);
pub const DUR_ENTER: Duration = Duration::from_millis(180);
pub const DUR_OVERLAY: Duration = Duration::from_millis(220);
pub const DUR_TOAST: Duration = Duration::from_millis(240);

/// `cubic-bezier(0.2, 0, 0, 1)`: enter / state changes.
pub fn ease_out(t: f32) -> f32 {
    cubic_bezier(0.2, 0.0, 0.0, 1.0, t)
}

/// `cubic-bezier(0.4, 0, 1, 1)`: exits.
pub fn ease_in(t: f32) -> f32 {
    cubic_bezier(0.4, 0.0, 1.0, 1.0, t)
}

/// CSS-style cubic bezier timing function.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let bez = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    // Bisection on x(t); 20 steps is far below a pixel of error.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut t = x;
    for _ in 0..20 {
        let xt = bez(x1, x2, t);
        if (xt - x).abs() < 1e-4 {
            break;
        }
        if xt < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    bez(y1, y2, t).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

struct Fonts {
    sans: SharedString,
    mono: SharedString,
}

static FONTS: OnceLock<Fonts> = OnceLock::new();

/// Picks Inter / JetBrains Mono when installed, with sensible fallbacks. Call once at startup.
pub fn init_fonts(cx: &App) {
    let names = cx.text_system().all_font_names();
    let pick = |prefs: &[&str], fallback: &str| -> SharedString {
        prefs
            .iter()
            .find(|p| names.iter().any(|n| n == *p))
            .map(|s| SharedString::from(s.to_string()))
            .unwrap_or_else(|| SharedString::from(fallback.to_string()))
    };
    let sans = pick(&["Inter", "Inter Variable"], ".SystemUIFont");
    let mono = pick(
        &["JetBrains Mono", "JetBrainsMono Nerd Font", "Geist Mono", "DejaVu Sans Mono", "Menlo", "Consolas"],
        "monospace",
    );
    let _ = FONTS.set(Fonts { sans, mono });
}

/// UI font family.
pub fn sans() -> SharedString {
    FONTS.get().map(|f| f.sans.clone()).unwrap_or_else(|| ".SystemUIFont".into())
}

/// Monospace font family (ids, code, diffs, numbers in tables).
pub fn mono() -> SharedString {
    FONTS.get().map(|f| f.mono.clone()).unwrap_or_else(|| "monospace".into())
}

// ---------------------------------------------------------------------------
// gpui-component theme
// ---------------------------------------------------------------------------

/// Maps our tokens onto gpui-component's theme so its Input, Select, Tooltip, Scrollbar,
/// TextView... match the rest of the app. Call after `gpui_component::init` and `init_fonts`.
pub fn apply_component_theme(cx: &mut App) {
    use gpui_component::{Theme, ThemeMode};
    Theme::change(ThemeMode::Dark, None, cx);
    let t = Theme::global_mut(cx);
    t.font_family = sans();
    t.mono_font_family = mono();
    t.mono_font_size = px(12.);
    t.radius = RADIUS_LG;
    t.radius_lg = RADIUS_XL;
    t.shadow = false;

    let c = &mut t.colors;
    c.background = BG;
    c.foreground = TEXT;
    c.border = LINE_STRONG;
    c.input = LINE_STRONG;
    c.ring = ACCENT;
    c.caret = TEXT;
    c.selection = alpha(ACCENT, 0.35);
    c.muted = SURFACE_RAISED;
    c.muted_foreground = TEXT_MUTED;
    c.accent = SURFACE_HOVER;
    c.accent_foreground = TEXT;
    c.primary = ACCENT_STRONG;
    c.primary_hover = ACCENT_STRONG_HOVER;
    c.primary_active = ACCENT_STRONG_HOVER;
    c.primary_foreground = ON_ACCENT;
    c.secondary = SURFACE_RAISED;
    c.secondary_hover = SURFACE_HOVER;
    c.secondary_active = SURFACE_HOVER;
    c.secondary_foreground = TEXT;
    c.popover = SURFACE_RAISED;
    c.popover_foreground = TEXT;
    c.list = SURFACE;
    c.list_even = SURFACE;
    c.list_head = SURFACE;
    c.list_hover = SURFACE_HOVER;
    c.list_active = ACCENT_SOFT;
    c.list_active_border = ACCENT_RING;
    c.table = SURFACE;
    c.table_even = SURFACE;
    c.table_head = SURFACE;
    c.table_head_foreground = TEXT_MUTED;
    c.table_hover = SURFACE_RAISED;
    c.table_active = ACCENT_SOFT;
    c.table_active_border = ACCENT_RING;
    c.table_row_border = LINE;
    c.tab = TRANSPARENT;
    c.tab_active = TRANSPARENT;
    c.tab_active_foreground = TEXT;
    c.tab_bar = TRANSPARENT;
    c.tab_foreground = TEXT_MUTED;
    c.scrollbar = TRANSPARENT;
    c.scrollbar_thumb = LINE_STRONG;
    c.scrollbar_thumb_hover = hex(0x3a3d44);
    c.skeleton = SURFACE_RAISED;
    c.overlay = SCRIM;
    c.window_border = LINE;
    c.title_bar = BG_SIDEBAR;
    c.title_bar_border = LINE;
    c.sidebar = BG_SIDEBAR;
    c.sidebar_border = LINE;
    c.sidebar_foreground = TEXT_SECONDARY;
    c.sidebar_accent = SURFACE_HOVER;
    c.sidebar_accent_foreground = TEXT;
    c.sidebar_primary = ACCENT_STRONG;
    c.sidebar_primary_foreground = ON_ACCENT;
    c.progress_bar = ACCENT;
    c.slider_bar = ACCENT;
    c.slider_thumb = TEXT;
    c.switch = LINE_STRONG;
    c.link = ACCENT_TEXT;
    c.link_hover = ACCENT;
    c.link_active = ACCENT;
    c.success = SUCCESS;
    c.success_hover = SUCCESS;
    c.success_active = SUCCESS;
    c.success_foreground = BG;
    c.warning = WARNING;
    c.warning_hover = WARNING;
    c.warning_active = WARNING;
    c.warning_foreground = BG;
    c.danger = DANGER;
    c.danger_hover = DANGER;
    c.danger_active = DANGER;
    c.danger_foreground = ON_ACCENT;
    c.info = INFO;
    c.info_hover = INFO;
    c.info_active = INFO;
    c.info_foreground = BG;
    c.chart_1 = ACCENT;
    c.chart_2 = INFO;
    c.chart_3 = SUCCESS;
    c.chart_4 = WARNING;
    c.chart_5 = SPAWN;
    c.description_list_label = SURFACE;
    c.description_list_label_foreground = TEXT_MUTED;
    c.group_box = SURFACE;
    c.group_box_foreground = TEXT;
    c.drag_border = ACCENT;
    c.drop_target = alpha(ACCENT, 0.12);
}

// ---------------------------------------------------------------------------
// Text styles
// ---------------------------------------------------------------------------

/// Type scale from the design (`--text-*`). Size + line height + weight (+ family for mono).
/// Color helpers (`fg_*`) are separate so they compose.
pub trait TextStyles: Styled + Sized {
    /// 13/20 400.
    fn text_body(self) -> Self {
        self.text_size(px(13.)).line_height(px(20.)).font_weight(FontWeight::NORMAL)
    }
    /// 13/20 500.
    fn text_body_strong(self) -> Self {
        self.text_size(px(13.)).line_height(px(20.)).font_weight(FontWeight::MEDIUM)
    }
    /// 14/20 500. Card / section headings.
    fn text_heading(self) -> Self {
        self.text_size(px(14.)).line_height(px(20.)).font_weight(FontWeight::MEDIUM)
    }
    /// 12/18 400. Labels, captions, meta.
    fn text_label(self) -> Self {
        self.text_size(px(12.)).line_height(px(18.)).font_weight(FontWeight::NORMAL)
    }
    /// 12/20 mono.
    fn text_mono(self) -> Self {
        self.font_family(mono()).text_size(px(12.)).line_height(px(20.)).font_weight(FontWeight::NORMAL)
    }
    /// 11/16 mono.
    fn text_mono_sm(self) -> Self {
        self.font_family(mono()).text_size(px(11.)).line_height(px(16.)).font_weight(FontWeight::NORMAL)
    }
    /// 28/32 600. Big numbers in stat rows.
    fn text_stat(self) -> Self {
        self.text_size(px(28.)).line_height(px(32.)).font_weight(FontWeight::SEMIBOLD)
    }
    /// 18/24 500. Small stat values.
    fn text_stat_sm(self) -> Self {
        self.text_size(px(18.)).line_height(px(24.)).font_weight(FontWeight::MEDIUM)
    }
    /// 20/28 600. Page / item titles.
    fn text_title(self) -> Self {
        self.text_size(px(20.)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD)
    }
    /// 22/28 600.
    fn text_title_lg(self) -> Self {
        self.text_size(px(22.)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD)
    }

    fn fg(self) -> Self {
        self.text_color(TEXT)
    }
    fn fg_secondary(self) -> Self {
        self.text_color(TEXT_SECONDARY)
    }
    fn fg_muted(self) -> Self {
        self.text_color(TEXT_MUTED)
    }
    fn fg_disabled(self) -> Self {
        self.text_color(TEXT_DISABLED)
    }
    fn fg_accent(self) -> Self {
        self.text_color(ACCENT_TEXT)
    }
}

impl<T: Styled + Sized> TextStyles for T {}

// ---------------------------------------------------------------------------
// Domain colors
// ---------------------------------------------------------------------------

/// Dot / text color for a task or run status.
pub fn status_color(s: Status) -> Hsla {
    match s {
        Status::Queued => TEXT_MUTED,
        Status::Running => INFO,
        Status::Waiting => WARNING,
        Status::Done => SUCCESS,
        Status::Failed => DANGER,
        Status::Cancelled => TEXT_DISABLED,
    }
}

/// Soft background matching [`status_color`] (pills, highlighted cards).
pub fn status_soft(s: Status) -> Hsla {
    match s {
        Status::Queued | Status::Cancelled => SURFACE_RAISED,
        Status::Running => INFO_SOFT,
        Status::Waiting => WARNING_SOFT,
        Status::Done => SUCCESS_SOFT,
        Status::Failed => DANGER_SOFT,
    }
}

/// A pipeline's sidebar dot color (`Pipeline::color`, `0xRRGGBB`).
pub fn rgb_u32(c: u32) -> Hsla {
    hex(c)
}
