use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, pulsating_between, px, Animation, AnimationExt as _, AnyElement, App, Div, ElementId,
    Hsla, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window,
};
use harness_core::Status;

use crate::theme::{self, TextStyles as _};

// ---------------------------------------------------------------------------
// Badge
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BadgeTone {
    /// Outlined, secondary text ("v14").
    #[default]
    Neutral,
    /// Soft accent fill ("Draft v15").
    Accent,
    /// Soft violet fill (spawned sub-agents).
    Spawn,
    Success,
    Warning,
    Danger,
    Info,
}

/// A 22px pill. `Badge::new("Draft v15").accent()`, `Badge::new("3 waiting").warning().dot()`.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: BadgeTone,
    dot: bool,
    mono: bool,
    leading: Option<AnyElement>,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Badge {
            label: label.into(),
            tone: BadgeTone::Neutral,
            dot: false,
            mono: false,
            leading: None,
        }
    }
    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.tone = tone;
        self
    }
    pub fn accent(self) -> Self {
        self.tone(BadgeTone::Accent)
    }
    pub fn spawn(self) -> Self {
        self.tone(BadgeTone::Spawn)
    }
    pub fn success(self) -> Self {
        self.tone(BadgeTone::Success)
    }
    pub fn warning(self) -> Self {
        self.tone(BadgeTone::Warning)
    }
    pub fn danger(self) -> Self {
        self.tone(BadgeTone::Danger)
    }
    pub fn info(self) -> Self {
        self.tone(BadgeTone::Info)
    }
    /// Leading 6px dot in the tone color.
    pub fn dot(mut self) -> Self {
        self.dot = true;
        self
    }
    /// Monospace label (ids like `PAY-1423`, versions).
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    /// Arbitrary leading element (an icon, a [`StatusDot`]).
    pub fn leading(mut self, el: impl IntoElement) -> Self {
        self.leading = Some(el.into_any_element());
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (bg, border, fg, dot) = match self.tone {
            BadgeTone::Neutral => (
                theme::TRANSPARENT,
                theme::LINE_STRONG,
                theme::TEXT_SECONDARY,
                theme::TEXT_MUTED,
            ),
            BadgeTone::Accent => (
                theme::ACCENT_SOFT,
                theme::TRANSPARENT,
                theme::ACCENT_TEXT,
                theme::ACCENT,
            ),
            BadgeTone::Spawn => (
                theme::SPAWN_SOFT,
                theme::TRANSPARENT,
                theme::SPAWN_TEXT,
                theme::SPAWN,
            ),
            BadgeTone::Success => (
                theme::TRANSPARENT,
                theme::LINE_STRONG,
                theme::SUCCESS,
                theme::SUCCESS,
            ),
            BadgeTone::Warning => (
                theme::TRANSPARENT,
                theme::LINE_STRONG,
                theme::WARNING,
                theme::WARNING,
            ),
            BadgeTone::Danger => (
                theme::TRANSPARENT,
                theme::LINE_STRONG,
                theme::DANGER,
                theme::DANGER,
            ),
            BadgeTone::Info => (
                theme::TRANSPARENT,
                theme::LINE_STRONG,
                theme::INFO_TEXT,
                theme::INFO,
            ),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(22.))
            .px(px(8.))
            .rounded(theme::RADIUS_FULL)
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .text_size(px(12.))
            .line_height(px(16.))
            .whitespace_nowrap()
            .when(self.mono, |d| d.font_family(theme::mono()))
            .when(self.dot, |d| d.child(dot_el(dot, px(6.))))
            .children(self.leading)
            .child(self.label)
    }
}

/// Accent count pill (sidebar "6", tab counts).
pub fn count_pill(n: usize) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(6.))
        .rounded(theme::RADIUS_FULL)
        .bg(theme::ACCENT_STRONG)
        .text_color(theme::ON_ACCENT)
        .text_size(px(11.))
        .line_height(px(16.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(n.to_string())
}

/// Keyboard hint (`⌘K`, `J`, `↵`).
pub fn kbd(keys: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(5.))
        .rounded(theme::RADIUS_SM)
        .border_1()
        .border_color(theme::LINE_STRONG)
        .text_mono_sm()
        .text_color(theme::TEXT_MUTED)
        .child(keys.into())
}

/// A plain filled dot.
pub fn dot_el(color: Hsla, size: gpui::Pixels) -> Div {
    div()
        .flex_none()
        .size(size)
        .rounded(theme::RADIUS_FULL)
        .bg(color)
}

/// A hollow ring dot (queued, draft, not deployed).
pub fn ring_el(color: Hsla, size: gpui::Pixels) -> Div {
    div()
        .flex_none()
        .size(size)
        .rounded(theme::RADIUS_FULL)
        .border(px(1.5))
        .border_color(color)
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// 8px status dot. Running pulses; queued and cancelled are hollow.
///
/// Give each dot in a list its own id (`StatusDot::new(s).id(("dot", ix))`) so their
/// animations don't share state.
#[derive(IntoElement)]
pub struct StatusDot {
    status: Status,
    id: ElementId,
    size: gpui::Pixels,
}

impl StatusDot {
    pub fn new(status: Status) -> Self {
        StatusDot {
            status,
            id: "status-dot".into(),
            size: px(8.),
        }
    }
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
    pub fn small(mut self) -> Self {
        self.size = px(6.);
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let color = theme::status_color(self.status);
        match self.status {
            Status::Queued | Status::Cancelled => ring_el(color, self.size).into_any_element(),
            Status::Running => {
                // A soft halo breathing behind a solid core.
                let size = self.size;
                div()
                    .flex_none()
                    .relative()
                    .size(size)
                    .child(
                        dot_el(theme::alpha(color, 0.35), size)
                            .absolute()
                            .top_0()
                            .left_0()
                            .with_animation(
                                self.id.clone(),
                                Animation::new(Duration::from_millis(1600))
                                    .repeat()
                                    .with_easing(pulsating_between(0.0, 1.0)),
                                move |d, t| {
                                    let grow = size * (0.9 * t);
                                    d.size(size + grow * 2.)
                                        .top(-grow)
                                        .left(-grow)
                                        .opacity(0.9 * (1.0 - t))
                                },
                            ),
                    )
                    .child(dot_el(color, size).absolute().top_0().left_0())
                    .into_any_element()
            }
            _ => dot_el(color, self.size).into_any_element(),
        }
    }
}

/// Dot + status label pill ("● Waiting for you").
#[derive(IntoElement)]
pub struct StatusPill {
    status: Status,
    id: ElementId,
    suffix: Option<SharedString>,
}

impl StatusPill {
    pub fn new(status: Status) -> Self {
        StatusPill {
            status,
            id: "status-pill".into(),
            suffix: None,
        }
    }
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
    /// Appended after the label: `"6m"` -> "Waiting for you · 6m".
    /// Text after the status ("Running · 3m"). Empty text adds nothing (no stray "·").
    pub fn suffix(mut self, s: impl Into<SharedString>) -> Self {
        let s: SharedString = s.into();
        self.suffix = (!s.trim().is_empty()).then_some(s);
        self
    }
}

impl RenderOnce for StatusPill {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let label: SharedString = match &self.suffix {
            Some(s) => format!("{} · {}", self.status.label(), s).into(),
            None => self.status.label().into(),
        };
        let border = match self.status {
            Status::Waiting => theme::alpha(theme::WARNING, 0.35),
            Status::Running => theme::alpha(theme::INFO, 0.35),
            Status::Failed => theme::alpha(theme::DANGER, 0.35),
            _ => theme::LINE_STRONG,
        };
        let fg = match self.status {
            Status::Queued => theme::TEXT_SECONDARY,
            Status::Cancelled => theme::TEXT_MUTED,
            Status::Running => theme::INFO_TEXT,
            s => theme::status_color(s),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(22.))
            .px(px(8.))
            .rounded(theme::RADIUS_FULL)
            .border_1()
            .border_color(border)
            .text_color(fg)
            .text_size(px(12.))
            .line_height(px(16.))
            .whitespace_nowrap()
            .child(StatusDot::new(self.status).id(self.id).small())
            .child(label)
    }
}

// ---------------------------------------------------------------------------
// Variant / letter chip
// ---------------------------------------------------------------------------

/// 24px letter tile: fan-out variants (A/B/C), agent initials, the app logo.
#[derive(IntoElement)]
pub struct VariantChip {
    letter: SharedString,
    selected: bool,
    size: gpui::Pixels,
}

impl VariantChip {
    pub fn new(letter: impl Into<SharedString>) -> Self {
        VariantChip {
            letter: letter.into(),
            selected: false,
            size: px(24.),
        }
    }
    /// Variant index -> `A`, `B`, ...
    pub fn variant(index: u32) -> Self {
        Self::new(harness_core::variant_letter(index).to_string())
    }
    /// First letter of a name, uppercased.
    pub fn initial(name: &str) -> Self {
        Self::new(
            name.chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into()),
        )
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn size(mut self, size: gpui::Pixels) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for VariantChip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(self.size)
            .rounded(theme::RADIUS_MD)
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .when(self.selected, |d| {
                d.bg(theme::ACCENT_STRONG).text_color(theme::ON_ACCENT)
            })
            .when(!self.selected, |d| {
                d.bg(theme::SURFACE_HOVER)
                    .border_1()
                    .border_color(theme::LINE_STRONG)
                    .text_color(theme::TEXT)
            })
            .child(self.letter)
    }
}
