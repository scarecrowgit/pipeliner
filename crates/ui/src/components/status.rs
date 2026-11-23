//! Status and small labels: dots, badges, counts, tags, key caps, progress bars, chips that
//! say something about an item. None of them is interactive.

use gpui::{
    AnyElement, App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, prelude::*, px, relative,
};

use crate::assets::{IconName, icon};
use crate::layout::row;
use crate::theme::*;

/// The state a [`StatusDot`] (or a badge dot) stands for. Always pair a dot with a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Deployed and running on schedule, or done: `success`.
    Live,
    /// In progress right now: `info`.
    Running,
    /// Blocked on a human: `warning`.
    Waiting,
    /// Failed, over budget or denied: `danger`.
    Error,
    /// Created by another agent: `spawn`.
    Spawn,
    /// Paused or not deployed: a ring in `text-muted`.
    Draft,
    /// Nothing happening: `text-disabled`.
    Idle,
}

impl Status {
    /// The dot's colour (the ring colour for `Draft`).
    pub fn color(self) -> Hsla {
        match self {
            Status::Live => SUCCESS,
            Status::Running => INFO,
            Status::Waiting => WARNING,
            Status::Error => DANGER,
            Status::Spawn => SPAWN,
            Status::Draft => TEXT_MUTED,
            Status::Idle => TEXT_DISABLED,
        }
    }
}

/// An 8px state marker that goes before a pipeline name, a run or a table state; `small` is 6px.
#[derive(IntoElement)]
pub struct StatusDot {
    status: Status,
    small: bool,
}

impl StatusDot {
    /// A dot for `status`, 8px.
    pub fn new(status: Status) -> Self {
        Self { status, small: false }
    }

    /// 6px, for dots inside badges and footers.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let size = if self.small { 6. } else { 8. };
        let dot = div().size(px(size)).flex_none().rounded_full();
        match self.status {
            Status::Draft => dot.border(px(1.5)).border_color(self.status.color()),
            s => dot.bg(s.color()),
        }
    }
}

/// The colour family of a [`Badge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BadgeTone {
    /// Outline: facts like a version or a repo name.
    #[default]
    Neutral,
    /// Draft, edited or unsaved.
    Accent,
    /// Anything an agent created.
    Spawn,
    /// Text-only success on the outline.
    Success,
    /// Text-only warning on the outline: priority, waiting.
    Warning,
    /// Text-only danger on the outline.
    Danger,
}

/// A one-to-three-word status pill next to a title ("Live · v14", "High"). Variants: tone
/// (neutral, accent, spawn, success, warning, danger), pill or `square`, an optional status
/// dot or leading icon.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: BadgeTone,
    square: bool,
    dot: Option<Status>,
    icon: Option<IconName>,
}

impl Badge {
    /// A neutral pill reading `label`.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), tone: BadgeTone::Neutral, square: false, dot: None, icon: None }
    }

    /// Set the colour family.
    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.tone = tone;
        self
    }

    /// 6px corners instead of a pill, 20px tall (`.badge.sq`): repo hosts, issue type.
    pub fn square(mut self) -> Self {
        self.square = true;
        self
    }

    /// A 6px status dot before the label.
    pub fn dot(mut self, status: Status) -> Self {
        self.dot = Some(status);
        self
    }

    /// A 12px icon before the label.
    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (border, bg, fg) = match self.tone {
            BadgeTone::Neutral => (LINE_STRONG, TRANSPARENT, TEXT_SECONDARY),
            BadgeTone::Accent => (TRANSPARENT, ACCENT_SOFT, ACCENT_TEXT),
            BadgeTone::Spawn => (TRANSPARENT, SPAWN_SOFT, SPAWN_TEXT),
            BadgeTone::Success => (LINE_STRONG, TRANSPARENT, SUCCESS),
            BadgeTone::Warning => (LINE_STRONG, TRANSPARENT, WARNING),
            BadgeTone::Danger => (LINE_STRONG, TRANSPARENT, DANGER),
        };
        let (h, px_, radius) =
            if self.square { (20., SPACE_1_5, RADIUS_MD) } else { (22., SPACE_2, RADIUS_FULL) };
        row()
            .flex_none()
            .gap(px(SPACE_1_5))
            .h(px(h))
            .px(px(px_))
            .rounded(px(radius))
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .type_style(LABEL_TIGHT)
            .whitespace_nowrap()
            .when_some(self.dot, |d, s| d.child(StatusDot::new(s).small()))
            .when_some(self.icon, |d, i| d.child(icon(i, 12., fg)))
            .child(self.label)
    }
}

/// The colour family of a [`CountBadge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CountTone {
    /// Solid accent: counts that need action (reviews waiting).
    #[default]
    Accent,
    /// Quiet grey: totals and secondary counts.
    Quiet,
    /// Danger text on a danger fill: failures.
    Danger,
}

/// A solid number pill in nav and card headers ("4"). Show only counts that need action.
#[derive(IntoElement)]
pub struct CountBadge {
    text: SharedString,
    tone: CountTone,
}

impl CountBadge {
    /// An accent count showing `text`.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), tone: CountTone::Accent }
    }

    /// Grey instead of accent.
    pub fn quiet(mut self) -> Self {
        self.tone = CountTone::Quiet;
        self
    }

    /// Set the colour family.
    pub fn tone(mut self, tone: CountTone) -> Self {
        self.tone = tone;
        self
    }
}

impl RenderOnce for CountBadge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (bg, fg, style) = match self.tone {
            CountTone::Accent => (ACCENT_STRONG, ON_ACCENT, CAPTION_STRONG),
            CountTone::Quiet => (SURFACE_HOVER, TEXT_SECONDARY, TypeStyle { weight: gpui::FontWeight::MEDIUM, ..CAPTION }),
            CountTone::Danger => (DANGER_SOFT, DANGER, TypeStyle { weight: gpui::FontWeight::MEDIUM, ..CAPTION }),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .min_w(px(18.))
            .h(px(18.))
            .px(px(SPACE_1_5))
            .rounded_full()
            .bg(bg)
            .text_color(fg)
            .type_style(style)
            .tabular()
            .child(self.text)
    }
}

/// The colour family of a [`Tag`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TagTone {
    /// Grey.
    #[default]
    Neutral,
    /// Created by an agent.
    Spawn,
    /// Accent.
    Accent,
}

/// A small filled label for facts about an item (a branch, a label). `mono` for machine text.
#[derive(IntoElement)]
pub struct Tag {
    text: SharedString,
    tone: TagTone,
    mono: bool,
}

impl Tag {
    /// A neutral sans tag.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), tone: TagTone::Neutral, mono: false }
    }

    /// Set the colour family.
    pub fn tone(mut self, tone: TagTone) -> Self {
        self.tone = tone;
        self
    }

    /// Mono 11px, for branch names and keys.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

impl RenderOnce for Tag {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (bg, fg) = match self.tone {
            TagTone::Neutral => (SURFACE_HOVER, TEXT_SECONDARY),
            TagTone::Spawn => (SPAWN_SOFT, SPAWN_TEXT),
            TagTone::Accent => (ACCENT_SOFT, ACCENT_TEXT),
        };
        row()
            .flex_none()
            .gap(px(SPACE_1))
            .h(px(20.))
            .px(px(SPACE_1_5))
            .rounded(px(RADIUS_MD))
            .bg(bg)
            .text_color(fg)
            .type_style(if self.mono { MONO_SM } else { LABEL_TIGHT })
            .whitespace_nowrap()
            .child(self.text)
    }
}

/// A key cap in mono 11px on a `line-strong` outline: "⌘K". Shortcut hints at the foot of a
/// queue or inside a field. `on_accent` is the variant inside a primary button.
#[derive(IntoElement)]
pub struct Kbd {
    text: SharedString,
    on_accent: bool,
}

impl Kbd {
    /// A key cap showing `text`.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), on_accent: false }
    }

    /// White-on-accent colours, for a hint inside a primary button.
    pub fn on_accent(mut self) -> Self {
        self.on_accent = true;
        self
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (border, fg) = if self.on_accent { (ON_ACCENT_LINE, ON_ACCENT) } else { (LINE_STRONG, TEXT_MUTED) };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .min_w(px(18.))
            .h(px(18.))
            .px(px(5.))
            .border_1()
            .border_color(border)
            .rounded(px(RADIUS_SM))
            .text_color(fg)
            .type_style(MONO_SM)
            .child(self.text)
    }
}

/// A row of shortcut hints: key caps followed by what they do ("↑ ↓ move  ↵ open"). At the
/// foot of queues and pickers.
#[derive(IntoElement, Default)]
pub struct KeyHints {
    hints: Vec<(Vec<SharedString>, SharedString)>,
    wrap: bool,
}

impl KeyHints {
    /// An empty row.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a hint: the keys, then the verb.
    pub fn hint<K: Into<SharedString>>(
        mut self,
        keys: impl IntoIterator<Item = K>,
        label: impl Into<SharedString>,
    ) -> Self {
        self.hints.push((keys.into_iter().map(Into::into).collect(), label.into()));
        self
    }

    /// Let the hints wrap onto more lines. Do not use it next to wrapping text in a column.
    pub fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }
}

impl RenderOnce for KeyHints {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let items: Vec<AnyElement> = self
            .hints
            .into_iter()
            .map(|(keys, label)| {
                row()
                    .flex_none()
                    .gap(px(SPACE_1))
                    .children(keys.into_iter().map(Kbd::new))
                    .child(label)
                    .into_any_element()
            })
            .collect();
        row()
            .gap_x(px(SPACE_3_5))
            .gap_y(px(SPACE_1_5))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .when(self.wrap, |d| d.flex_wrap())
            .children(items)
    }
}

/// The colour of a [`Progress`] fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProgressTone {
    /// Accent: the default.
    #[default]
    Accent,
    /// Running.
    Info,
    /// Approaching a limit.
    Warning,
    /// Over a limit.
    Danger,
    /// Done.
    Success,
}

/// A 4px bar showing how far along something is. Fills its parent's width unless `width` is set.
#[derive(IntoElement)]
pub struct Progress {
    fraction: f32,
    tone: ProgressTone,
    width: Option<f32>,
}

impl Progress {
    /// A bar `fraction` (0 to 1) full.
    pub fn new(fraction: f32) -> Self {
        Self { fraction, tone: ProgressTone::Accent, width: None }
    }

    /// Set the fill colour.
    pub fn tone(mut self, tone: ProgressTone) -> Self {
        self.tone = tone;
        self
    }

    /// A fixed width in px instead of the parent's width (the rail's spend bar is 32).
    pub fn width(mut self, px: f32) -> Self {
        self.width = Some(px);
        self
    }
}

impl RenderOnce for Progress {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let fill = match self.tone {
            ProgressTone::Accent => ACCENT,
            ProgressTone::Info => INFO,
            ProgressTone::Warning => WARNING,
            ProgressTone::Danger => DANGER,
            ProgressTone::Success => SUCCESS,
        };
        div()
            .h(px(4.))
            .flex_none()
            .when_some(self.width, |d, w| d.w(px(w)))
            .when(self.width.is_none(), |d| d.w_full())
            .rounded_full()
            .bg(LINE)
            .overflow_hidden()
            .child(div().h_full().w(relative(self.fraction.clamp(0., 1.))).rounded_full().bg(fill))
    }
}

/// Marks a place where an agent creates agents ("Spawns Reproducer · 2 of 5"): `spawn-soft`
/// fill, `spawn-text` and the spawn icon at 12px. Violet means only "created by an agent".
#[derive(IntoElement)]
pub struct SpawnChip {
    text: SharedString,
    icon: bool,
}

impl SpawnChip {
    /// A chip reading `text`, with the spawn icon.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), icon: true }
    }

    /// Leave the icon out.
    pub fn no_icon(mut self) -> Self {
        self.icon = false;
        self
    }
}

impl RenderOnce for SpawnChip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        row()
            .flex_none()
            .gap(px(SPACE_1_5))
            .px(px(SPACE_2))
            .py(px(SPACE_1))
            .rounded(px(RADIUS_MD))
            .bg(SPAWN_SOFT)
            .text_color(SPAWN_TEXT)
            .type_style(LABEL_TIGHT)
            .whitespace_nowrap()
            .when(self.icon, |d| d.child(icon(IconName::Spawn, 12., SPAWN_TEXT)))
            .child(self.text)
    }
}

/// One condition of a router rule: a field (muted), an operator and a value (medium weight),
/// 28px tall. `edited` marks a condition that changed in the current draft.
#[derive(IntoElement)]
pub struct RuleChip {
    field: SharedString,
    op: SharedString,
    value: SharedString,
    edited: bool,
}

impl RuleChip {
    /// `field op value`, e.g. `RuleChip::new("type", "=", "Bug")`.
    pub fn new(
        field: impl Into<SharedString>,
        op: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self { field: field.into(), op: op.into(), value: value.into(), edited: false }
    }

    /// Accent look: the condition changed in the current draft.
    pub fn edited(mut self, edited: bool) -> Self {
        self.edited = edited;
        self
    }
}

impl RenderOnce for RuleChip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (border, bg, field) = if self.edited {
            (ACCENT_STRONG, ACCENT_SOFT, ACCENT_TEXT)
        } else {
            (LINE_STRONG, SURFACE_RAISED, TEXT_MUTED)
        };
        row()
            .flex_none()
            .gap(px(SPACE_1_5))
            .h(px(CONTROL_HEIGHT_SM))
            .px(px(SPACE_2_5))
            .border_1()
            .border_color(border)
            .rounded(px(RADIUS_MD))
            .bg(bg)
            .text_color(TEXT)
            .type_style(BODY)
            .whitespace_nowrap()
            .child(div().text_color(field).child(self.field))
            .child(self.op)
            .child(div().font_weight(gpui::FontWeight::MEDIUM).child(self.value))
    }
}
