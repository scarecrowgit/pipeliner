//! Graphite components (port of the mockup's design-system bundle).
//!
//! All are stateless `RenderOnce` builders; state lives in the screen entity and is
//! passed in (`selected`, `value`, ...) with callbacks for changes.
//!
//! ```ignore
//! Button::new("run", "Test run").icon("play").on_click(cx.listener(|this, _, _, cx| { .. }))
//! Badge::new("Draft v15").tone(Tone::Accent)
//! Stepper::new("depth", self.depth).range(1, 4).on_change(cx.listener(|this, v: &i32, _, cx| { .. }))
//! ```

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, SharedString, Svg,
    Window, div, prelude::*, px, relative, svg,
};

use crate::motion;
use crate::theme::*;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type IndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;
type ValueHandler = Rc<dyn Fn(&i32, &mut Window, &mut App) + 'static>;

/// Derive a child element id from a parent id: `"<parent>-<suffix>"`.
pub fn child_id(parent: &ElementId, suffix: &str) -> ElementId {
    ElementId::Name(format!("{parent}-{suffix}").into())
}

// ── Icon ────────────────────────────────────────────────────────────────

/// Stroke icon from `assets/icons/<name>.svg`, tinted with `color`.
/// Icons are 24×24 lucide-style outlines drawn in black (alpha mask).
pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

// ── Text helpers ────────────────────────────────────────────────────────

/// 12px JetBrains Mono.
pub fn mono(text: impl Into<SharedString>) -> gpui::Div {
    div().font_family(FONT_MONO).text_size(px(TEXT_MONO)).child(text.into())
}

/// 11px JetBrains Mono, line 16px.
pub fn mono_sm(text: impl Into<SharedString>) -> gpui::Div {
    div().font_family(FONT_MONO).text_size(px(TEXT_MONO_SM)).line_height(px(16.)).child(text.into())
}

/// 12px muted label.
pub fn label(text: impl Into<SharedString>) -> gpui::Div {
    div().text_size(px(TEXT_LABEL)).line_height(px(18.)).text_color(TEXT_MUTED).child(text.into())
}

/// Horizontal 1px separator.
pub fn hr() -> gpui::Div {
    div().h(px(1.)).w_full().flex_none().bg(LINE)
}

/// Vertical 1px separator.
pub fn vr() -> gpui::Div {
    div().w(px(1.)).h_full().flex_none().bg(LINE)
}

/// `.gr-card` container: surface bg, 1px line border, radius-xl, 14px padding, column gap 10.
pub fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .bg(SURFACE)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
}

/// 24×24 rounded icon tile used in card heads (`.gr-card__icon`).
pub fn icon_tile(name: &str, tone: Tone) -> gpui::Div {
    let (bg, fg) = match tone {
        Tone::Info => (INFO_SOFT, INFO_TEXT),
        Tone::Spawn => (SPAWN_SOFT, SPAWN_TEXT),
        Tone::Neutral => (SURFACE_HOVER, TEXT_SECONDARY),
        _ => (ACCENT_SOFT, ACCENT_TEXT),
    };
    div()
        .size(px(24.))
        .flex_none()
        .rounded(px(RADIUS_MD))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(name, 14., fg))
}

/// Initials avatar (e.g. "AR"), 20px circle.
pub fn avatar(initials: &str, size: f32) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(SURFACE_HOVER)
        .border_1()
        .border_color(LINE_STRONG)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size * 0.45).max(9.)))
        .font_weight(FontWeight::MEDIUM)
        .text_color(TEXT_SECONDARY)
        .child(SharedString::from(initials.to_string()))
}

// ── Tone ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Spawn,
    Success,
    Warning,
    Danger,
    Info,
    /// Filled count pill (sidebar "4").
    Count,
}

// ── Status dot ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Live,
    Running,
    Waiting,
    Error,
    Spawn,
    /// Hollow ring.
    Draft,
}

impl Status {
    pub fn color(self) -> Hsla {
        match self {
            Status::Live => SUCCESS,
            Status::Running => INFO,
            Status::Waiting => WARNING,
            Status::Error => DANGER,
            Status::Spawn => SPAWN,
            Status::Draft => TEXT_MUTED,
        }
    }
}

/// 8px (or 6px when `small`) status dot.
pub fn status_dot(status: Status, small: bool) -> gpui::Div {
    let s = if small { 6. } else { 8. };
    let d = div().size(px(s)).flex_none().rounded_full();
    if status == Status::Draft {
        d.border(px(1.5)).border_color(TEXT_MUTED)
    } else {
        d.bg(status.color())
    }
}

// ── Badge ───────────────────────────────────────────────────────────────

#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: Tone,
    dot: Option<Status>,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), tone: Tone::Neutral, dot: None }
    }
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
    pub fn dot(mut self, status: Status) -> Self {
        self.dot = Some(status);
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let base = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(22.))
            .px(px(8.))
            .rounded_full()
            .border_1()
            .border_color(LINE_STRONG)
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(TEXT_SECONDARY)
            .whitespace_nowrap();
        let base = match self.tone {
            Tone::Accent => base.border_color(gpui::transparent_black()).bg(ACCENT_SOFT).text_color(ACCENT_TEXT),
            Tone::Spawn => base.border_color(gpui::transparent_black()).bg(SPAWN_SOFT).text_color(SPAWN_TEXT),
            Tone::Info => base.border_color(gpui::transparent_black()).bg(INFO_SOFT).text_color(INFO_TEXT),
            Tone::Success => base.text_color(SUCCESS),
            Tone::Warning => base.text_color(WARNING),
            Tone::Danger => base.text_color(DANGER),
            Tone::Count => base
                .h(px(18.))
                .px(px(7.))
                .border_0()
                .bg(ACCENT_STRONG)
                .text_color(ON_ACCENT)
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD),
            Tone::Neutral => base,
        };
        base.when_some(self.dot, |d, s| d.child(status_dot(s, true))).child(self.label)
    }
}

// ── Kbd ─────────────────────────────────────────────────────────────────

pub fn kbd(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(5.))
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(RADIUS_SM))
        .font_family(FONT_MONO)
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(TEXT_MUTED)
        .child(text.into())
}

// ── Button ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 40px tall, 13px.
    #[default]
    Md,
    /// 28px tall, 12px.
    Sm,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    variant: ButtonVariant,
    size: ButtonSize,
    icon: Option<SharedString>,
    disabled: bool,
    selected: bool,
    full_width: bool,
    on_click: Option<ClickHandler>,
    leading: Option<AnyElement>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let label: SharedString = label.into();
        Self {
            id: id.into(),
            label: if label.is_empty() { None } else { Some(label) },
            variant: ButtonVariant::Secondary,
            size: ButtonSize::Md,
            icon: None,
            disabled: false,
            selected: false,
            full_width: false,
            on_click: None,
            leading: None,
        }
    }
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }
    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }
    pub fn small(mut self) -> Self {
        self.size = ButtonSize::Sm;
        self
    }
    /// Icon name from `assets/icons`.
    pub fn icon(mut self, name: impl Into<SharedString>) -> Self {
        self.icon = Some(name.into());
        self
    }
    /// Arbitrary leading element (instead of a named icon).
    pub fn leading(mut self, el: impl IntoElement) -> Self {
        self.leading = Some(el.into_any_element());
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
    /// Toggled-on look (accent border + soft bg), for toggle chips.
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let small = self.size == ButtonSize::Sm;
        let (h, pad, font, radius, icon_size) =
            if small { (28., 10., 12., RADIUS_MD, 12.) } else { (40., 14., 13., RADIUS_LG, 14.) };
        let pad = match self.variant {
            ButtonVariant::Primary if !small => 16.,
            ButtonVariant::Ghost | ButtonVariant::Danger if !small => 12.,
            _ => pad,
        };
        let (bg, hover_bg, border, fg, hover_fg) = match self.variant {
            ButtonVariant::Primary => (ACCENT_STRONG, ACCENT_STRONG_HOVER, None, ON_ACCENT, ON_ACCENT),
            ButtonVariant::Secondary => (SURFACE_RAISED, SURFACE_HOVER, Some(LINE_STRONG), TEXT, TEXT),
            ButtonVariant::Ghost => (gpui::transparent_black(), SURFACE_RAISED, None, TEXT_SECONDARY, TEXT),
            ButtonVariant::Danger => (gpui::transparent_black(), DANGER_SOFT, None, DANGER, DANGER),
        };
        let (bg, border, fg) =
            if self.selected { (ACCENT_SOFT, Some(ACCENT_STRONG), ACCENT_TEXT) } else { (bg, border, fg) };
        let disabled = self.disabled;
        let icon_only = self.label.is_none();

        // `.gr-btn`: background, border and colour fade over `duration-state`.
        let border = border.unwrap_or(gpui::transparent_black());
        let rest = motion::Look::bg(bg).fg(fg).border(border);
        let hover = if disabled || self.selected {
            rest
        } else {
            motion::Look::bg(hover_bg).fg(hover_fg).border(border)
        };
        let key = self.id.clone();
        let el = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(h))
            .when(icon_only, |d| d.w(px(h)))
            .when(!icon_only, |d| d.px(px(pad)))
            .when(self.full_width, |d| d.w_full())
            .rounded(px(radius))
            .border_1()
            .text_size(px(font))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(disabled, |d| d.opacity(0.45).cursor_not_allowed())
            .when(!disabled, |d| d.cursor_pointer());
        motion::hover_fade(el, key, rest, hover, window, cx)
            .when_some(self.leading, |d, el| d.child(el))
            .when_some(self.icon, |d, name| d.child(icon(&name, icon_size, fg)))
            .when_some(self.label, |d, l| d.child(l))
            .when_some(self.on_click.filter(|_| !disabled), |d, f| {
                d.on_click(move |e, w, cx| f(e, w, cx))
            })
    }
}

// ── Tabs ────────────────────────────────────────────────────────────────

pub struct TabItem {
    pub label: SharedString,
    pub count: Option<SharedString>,
}

impl TabItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), count: None }
    }
    pub fn count(mut self, c: impl Into<SharedString>) -> Self {
        self.count = Some(c.into());
        self
    }
}

/// Underline tab bar (`.gr-tabs`): 44px tall, gap 20, accent underline on selected.
/// Padding is left to the caller (mockup uses 0 24px).
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    items: Vec<TabItem>,
    selected: usize,
    on_select: Option<IndexHandler>,
    height: f32,
}

impl Tabs {
    pub fn new(id: impl Into<ElementId>, items: Vec<TabItem>, selected: usize) -> Self {
        Self { id: id.into(), items, selected, on_select: None, height: TAB_HEIGHT }
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Tabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let tabs: Vec<_> = self
            .items
            .into_iter()
            .enumerate()
            .map(|(i, it)| {
                let sel = i == self.selected;
                let cb = self.on_select.clone();
                let tab_id = child_id(&id, &i.to_string());
                let el = div()
                    .id(tab_id.clone())
                    .flex()
                    .items_center()
                    .h_full()
                    .border_b_2()
                    .border_color(if sel { ACCENT } else { gpui::transparent_black() })
                    .when(sel, |d| d.font_weight(FontWeight::MEDIUM))
                    .cursor_pointer();
                // `.gr-tab`: the label colour fades over `duration-state`.
                let clear = gpui::transparent_black();
                let rest = motion::Look::bg(clear).fg(if sel { TEXT } else { TEXT_MUTED });
                let hover = motion::Look::bg(clear).fg(TEXT);
                motion::hover_fade(el, tab_id, rest, hover, window, cx)
                    .child(it.label)
                    .when_some(it.count, |d, c| {
                        d.child(
                            div()
                                .ml(px(4.))
                                .text_color(TEXT_MUTED)
                                .font_weight(FontWeight::NORMAL)
                                .child(c),
                        )
                    })
                    .when_some(cb, |d, cb| d.on_click(move |_, w, cx| cb(i, w, cx)))
            })
            .collect();
        div()
            .id(self.id)
            .flex()
            .gap(px(SPACE_5))
            .h(px(self.height))
            .children(tabs)
    }
}

// ── Stat ────────────────────────────────────────────────────────────────

#[derive(IntoElement)]
pub struct Stat {
    label: SharedString,
    value: SharedString,
    sub: Option<SharedString>,
    large: bool,
    tone: Tone,
}

impl Stat {
    pub fn new(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self { label: label.into(), value: value.into(), sub: None, large: false, tone: Tone::Neutral }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn sub(mut self, s: impl Into<SharedString>) -> Self {
        self.sub = Some(s.into());
        self
    }
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }
    pub fn tone(mut self, t: Tone) -> Self {
        self.tone = t;
        self
    }
}

impl RenderOnce for Stat {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (size, line, weight) =
            if self.large { (28., 32., FontWeight::SEMIBOLD) } else { (18., 24., FontWeight::MEDIUM) };
        let value_color = if self.tone == Tone::Warning { WARNING } else { TEXT };
        let sub_color = if self.tone == Tone::Spawn { SPAWN_TEXT } else { TEXT_MUTED };
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(label(self.label))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(size))
                            .line_height(px(line))
                            .font_weight(weight)
                            .text_color(value_color)
                            .child(self.value),
                    )
                    .when_some(self.sub, |d, s| {
                        d.child(div().text_size(px(12.)).text_color(sub_color).child(s))
                    }),
            )
    }
}

// ── Rule chip ───────────────────────────────────────────────────────────

/// `field op **value**` chip (28px). `edited` = accent look.
pub fn rule_chip(field: &str, op: &str, value: &str, edited: bool) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(CONTROL_HEIGHT_SM))
        .px(px(10.))
        .border_1()
        .border_color(if edited { ACCENT_STRONG } else { LINE_STRONG })
        .rounded(px(RADIUS_MD))
        .bg(if edited { ACCENT_SOFT } else { SURFACE_RAISED })
        .text_color(TEXT)
        .whitespace_nowrap()
        .child(div().text_color(if edited { ACCENT_TEXT } else { TEXT_MUTED }).child(field.to_string()))
        .child(op.to_string())
        .child(div().font_weight(FontWeight::MEDIUM).child(value.to_string()))
}

// ── Stepper ─────────────────────────────────────────────────────────────

/// `− 3 +` stepper, 32px tall. Callback receives the new clamped value.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    value: i32,
    min: i32,
    max: i32,
    on_change: Option<ValueHandler>,
}

impl Stepper {
    pub fn new(id: impl Into<ElementId>, value: i32) -> Self {
        Self { id: id.into(), value, min: 1, max: 9, on_change: None }
    }
    pub fn range(mut self, min: i32, max: i32) -> Self {
        self.min = min;
        self.max = max;
        self
    }
    pub fn on_change(mut self, f: impl Fn(&i32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (v, min, max) = (self.value, self.min, self.max);
        let id = self.id.clone();
        let btn = |key: &'static str, glyph: &'static str, target: i32, enabled: bool| {
            let cb = self.on_change.clone();
            div()
                .id(child_id(&id, key))
                .w(px(32.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_MD))
                .text_color(TEXT_MUTED)
                .when(!enabled, |d| d.opacity(0.45).cursor_not_allowed())
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.text_color(TEXT)))
                .child(glyph)
                .when_some(cb.filter(|_| enabled), move |d, cb| {
                    d.on_click(move |_, w, cx| cb(&target, w, cx))
                })
        };
        div()
            .id(self.id.clone())
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_MD))
            .bg(SURFACE_RAISED)
            .child(btn("dec", "−", (v - 1).max(min), v > min))
            .child(div().min_w(px(20.)).flex().justify_center().child(SharedString::from(v.to_string())))
            .child(btn("inc", "+", (v + 1).min(max), v < max))
    }
}

// ── Progress ────────────────────────────────────────────────────────────

/// 4px progress bar; `fraction` in 0..=1. Fills its parent's width.
pub fn progress(fraction: f32, color: Hsla) -> gpui::Div {
    div()
        .h(px(4.))
        .w_full()
        .rounded_full()
        .bg(LINE)
        .overflow_hidden()
        .child(div().h_full().w(relative(fraction.clamp(0., 1.))).bg(color))
}

// ── Spawn chip ──────────────────────────────────────────────────────────

pub fn spawn_chip(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(RADIUS_MD))
        .bg(SPAWN_SOFT)
        .text_color(SPAWN_TEXT)
        .text_size(px(12.))
        .line_height(px(16.))
        .child(icon("spawn", 12., SPAWN_TEXT))
        .child(text.into())
}

// ── Checkbox ────────────────────────────────────────────────────────────

/// 16px checkbox; accent fill when checked. Wrap with your own label row.
pub fn checkbox(id: impl Into<ElementId>, checked: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .size(px(16.))
        .flex_none()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(if checked { ACCENT_STRONG } else { LINE_STRONG })
        .bg(if checked { ACCENT_STRONG } else { SURFACE_RAISED })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(checked, |d| d.child(icon("check", 12., ON_ACCENT)))
}

// ── Field shells ────────────────────────────────────────────────────────

/// Input-looking box (40px, surface-raised, line-strong border, radius-lg).
/// Put a `TextInput` entity or static text inside.
pub fn field_box() -> gpui::Div {
    div()
        .flex()
        .items_center()
        .h(px(CONTROL_HEIGHT))
        .px(px(12.))
        .bg(SURFACE_RAISED)
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(RADIUS_LG))
        .text_color(TEXT)
}

/// Select-looking box with a trailing chevron. Clicking is up to the caller
/// (e.g. open a popover menu).
pub fn select_box(id: impl Into<ElementId>, value: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    field_box()
        .id(id.into())
        .justify_between()
        .gap(px(8.))
        .cursor_pointer()
        .hover(|s| s.bg(SURFACE_HOVER))
        .child(div().flex_1().truncate().child(value.into()))
        .child(icon("chevron-down", 14., TEXT_MUTED))
}

/// Section title used in right-hand inspector panels (12px/500 muted-ish).
pub fn section_title(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(TEXT)
        .child(text.into())
}
