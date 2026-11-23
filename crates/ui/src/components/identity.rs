//! Identity marks: initials avatars, icon tiles and inline code.

use gpui::{App, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use crate::assets::{IconName, icon};
use crate::theme::*;

/// A round initials avatar: 20px, or `large` 24px. Initials are uppercase, two letters.
#[derive(IntoElement)]
pub struct Avatar {
    initials: SharedString,
    large: bool,
}

impl Avatar {
    /// An avatar showing `initials`.
    pub fn new(initials: impl Into<SharedString>) -> Self {
        Self { initials: initials.into(), large: false }
    }

    /// 24px instead of 20px.
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let size = if self.large { 24. } else { 20. };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(size))
            .rounded_full()
            .bg(SURFACE_HOVER)
            .border_1()
            .border_color(LINE_STRONG)
            .text_color(TEXT_SECONDARY)
            .type_style(if self.large { AVATAR_LG } else { AVATAR })
            .child(self.initials)
    }
}

/// The colour family of a [`Tile`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TileTone {
    /// Grey.
    #[default]
    Neutral,
    /// Accent.
    Accent,
    /// Data sources and nodes.
    Info,
    /// Agent-made.
    Spawn,
    /// Needs attention.
    Warn,
}

/// A rounded icon tile in a card head or menu row: 24px (14px icon), `medium` 28px in menu
/// rows (`.mi .tile`), or `large` 32px (16px icon, 8px radius).
#[derive(IntoElement)]
pub struct Tile {
    icon: IconName,
    tone: TileTone,
    large: bool,
    medium: bool,
}

impl Tile {
    /// A neutral tile showing `icon`.
    pub fn new(icon: IconName) -> Self {
        Self { icon, tone: TileTone::Neutral, large: false, medium: false }
    }

    /// Set the colour family.
    pub fn tone(mut self, tone: TileTone) -> Self {
        self.tone = tone;
        self
    }

    /// 32px instead of 24px.
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    /// 28px: the tile of a menu row.
    pub fn medium(mut self) -> Self {
        self.medium = true;
        self
    }
}

impl RenderOnce for Tile {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (bg, fg) = match self.tone {
            TileTone::Neutral => (SURFACE_HOVER, TEXT_SECONDARY),
            TileTone::Accent => (ACCENT_SOFT, ACCENT_TEXT),
            TileTone::Info => (INFO_SOFT, INFO_TEXT),
            TileTone::Spawn => (SPAWN_SOFT, SPAWN_TEXT),
            TileTone::Warn => (WARNING_SOFT, WARNING),
        };
        let (size, radius, glyph) = match (self.large, self.medium) {
            (true, _) => (32., RADIUS_LG, 16.),
            (false, true) => (28., RADIUS_MD, 14.),
            (false, false) => (24., RADIUS_MD, 14.),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(size))
            .rounded(px(radius))
            .bg(bg)
            .child(icon(self.icon, glyph, fg))
    }
}

/// Inline code: mono 12px on `surface-hover`, 4px radius. For commands, paths and keys inside
/// a sentence.
#[derive(IntoElement)]
pub struct Code {
    text: SharedString,
}

impl Code {
    /// A code span reading `text`.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for Code {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .px(px(5.))
            .py(px(1.))
            .rounded(px(RADIUS_SM))
            .bg(SURFACE_HOVER)
            .text_color(TEXT)
            .type_style(MONO)
            .whitespace_nowrap()
            .child(self.text)
    }
}
