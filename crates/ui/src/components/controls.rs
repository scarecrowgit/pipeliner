//! Form controls: toggle, checkbox, radio, stepper and the closed select trigger.

use std::rc::Rc;

use std::time::{Duration, Instant};

use gpui::{
    App, ClickEvent, ElementId, FocusHandle, IntoElement, RenderOnce, SharedString, Window, div,
    prelude::*, px,
};

use super::{ClickHandler, ValueHandler, child_id};
use crate::focus;
use crate::keys::{MoveDown, MoveUp};
use crate::assets::{IconName, icon};
use crate::layout::{row, truncate_lh};
use crate::motion::{self, Look};
use crate::theme::*;

/// An on/off switch, 28 by 16. The knob slides and the track fades over 150ms. The view owns
/// `on`; `on_change` receives the value the switch asks for.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    on: bool,
    disabled: bool,
    on_change: Option<ValueHandler<bool>>,
}

impl Toggle {
    /// A switch that is `on` or off.
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        Self { id: id.into(), on, disabled: false, on_change: None }
    }

    /// Dimmed, ignores clicks.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the opposite of the current value when the switch is clicked.
    pub fn on_change(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on = self.on;
        let track = motion::fade_color(
            child_id(&self.id, "track"),
            if on { ACCENT_STRONG } else { LINE_STRONG },
            window,
            cx,
        );
        let knob = motion::fade_color(
            child_id(&self.id, "knob"),
            if on { ON_ACCENT } else { TEXT_MUTED },
            window,
            cx,
        );
        let at = motion::fade_value(child_id(&self.id, "at"), if on { 1. } else { 0. }, window, cx);
        let selector = self.id.to_string();
        let disabled = self.disabled;
        let on_change = self.on_change.filter(|_| !disabled);
        let el = div().id(self.id).debug_selector(move || selector);
        focus::tab_stop(el, disabled, cx)
            .relative()
            .flex_none()
            .w(px(28.))
            .h(px(16.))
            .rounded_full()
            .bg(track)
            .when(disabled, |d| d.opacity(0.45).cursor_not_allowed())
            .when(!disabled, |d| d.cursor_pointer())
            .child(
                div()
                    .absolute()
                    .top(px(SPACE_0_5))
                    .left(px(SPACE_0_5 + 12. * at))
                    .size(px(12.))
                    .rounded_full()
                    .bg(knob),
            )
            .when_some(on_change, |d, f| d.on_click(move |_, w, cx| f(!on, w, cx)))
    }
}

/// The row around a checkbox or radio: the control and an optional label, one click target.
fn choice_row(
    id: ElementId,
    label: Option<SharedString>,
    disabled: bool,
    focus: Option<&FocusHandle>,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let selector = id.to_string();
    let el = row().id(id).debug_selector(move || selector);
    let el = match focus {
        Some(h) if !disabled => focus::track(el, h, cx),
        _ => focus::tab_stop(el, disabled, cx),
    };
    el.flex_none()
        .rounded(px(RADIUS_SM))
        .gap(px(SPACE_2))
        .when(label.is_some(), |d| d.type_style(BODY))
        .when(disabled, |d| d.opacity(0.45).cursor_not_allowed())
        .when(!disabled, |d| d.cursor_pointer())
}

/// A 16px checkbox with an optional label to its right. The view owns `checked`; `on_change`
/// receives the value the box asks for.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    label: Option<SharedString>,
    disabled: bool,
    on_change: Option<ValueHandler<bool>>,
}

impl Checkbox {
    /// A checkbox that is `checked` or not.
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self { id: id.into(), checked, label: None, disabled: false, on_change: None }
    }

    /// A label that is part of the click target.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Dimmed, ignores clicks.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the opposite of the current value when the checkbox is clicked.
    pub fn on_change(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let checked = self.checked;
        let look = if checked {
            Look::bg(ACCENT_STRONG).border(ACCENT_STRONG)
        } else {
            Look::bg(SURFACE_RAISED).border(LINE_STRONG)
        };
        let faded = motion::hover_fade_state(child_id(&self.id, "box"), look, look, window, cx);
        let boxed = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(16.))
            .rounded(px(RADIUS_SM))
            .border_1()
            .bg(faded.bg)
            .border_color(faded.border.unwrap_or(LINE_STRONG))
            .when(checked, |d| d.child(icon(IconName::Check, 10., ON_ACCENT)));
        let on_change = self.on_change.filter(|_| !self.disabled);
        choice_row(self.id, self.label.clone(), self.disabled, None, cx)
            .child(boxed)
            .when_some(self.label, |d, l| d.child(l))
            .when_some(on_change, |d, f| d.on_click(move |_, w, cx| f(!checked, w, cx)))
    }
}

/// A 16px radio with an optional label. Radios come in groups: the view owns which one is
/// `selected`, and `on_select` fires when an unselected one is clicked.
#[derive(IntoElement)]
pub struct Radio {
    id: ElementId,
    selected: bool,
    label: Option<SharedString>,
    disabled: bool,
    on_select: Option<ClickHandler>,
    /// Set by a [`RadioGroup`]: the group moves focus between its radios.
    focus: Option<FocusHandle>,
}

impl Radio {
    /// A radio that is `selected` or not.
    pub fn new(id: impl Into<ElementId>, selected: bool) -> Self {
        Self { id: id.into(), selected, label: None, disabled: false, on_select: None, focus: None }
    }

    /// A label that is part of the click target.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Dimmed, ignores clicks.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called when the radio is clicked while not selected.
    pub fn on_select(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Radio {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let selected = self.selected;
        let dot = radio_dot(&self.id, selected, window, cx);
        let on_select = self.on_select.filter(|_| !self.disabled && !selected);
        choice_row(self.id, self.label.clone(), self.disabled, self.focus.as_ref(), cx)
            .child(dot)
            .when_some(self.label, |d, l| d.child(l))
            .when_some(on_select, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

/// The 16px radio circle: a 5px accent ring around white when `selected`, fading.
pub(crate) fn radio_dot(id: &ElementId, selected: bool, window: &mut Window, cx: &mut App) -> gpui::Div {
    let look = if selected {
        Look::bg(ON_ACCENT).border(ACCENT_STRONG)
    } else {
        Look::bg(SURFACE_RAISED).border(LINE_STRONG)
    };
    let faded = motion::hover_fade_state(child_id(id, "dot"), look, look, window, cx);
    div()
        .flex_none()
        .size(px(16.))
        .rounded_full()
        .when(selected, |d| d.border(px(5.)))
        .when(!selected, |d| d.border_1())
        .bg(faded.bg)
        .border_color(faded.border.unwrap_or(LINE_STRONG))
}

/// Radios that belong together: one tab stop (the selected radio); arrow keys move the
/// selection. `on_select` receives the index of the radio to select.
#[derive(IntoElement)]
pub struct RadioGroup {
    id: ElementId,
    options: Vec<SharedString>,
    selected: usize,
    horizontal: bool,
    on_select: Option<ValueHandler<usize>>,
}

impl RadioGroup {
    /// An empty group.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), options: Vec::new(), selected: 0, horizontal: false, on_select: None }
    }

    /// Add a radio labelled `label`.
    pub fn option(mut self, label: impl Into<SharedString>) -> Self {
        self.options.push(label.into());
        self
    }

    /// Index of the selected radio.
    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    /// Lay the radios out in a row (they stack by default).
    pub fn horizontal(mut self) -> Self {
        self.horizontal = true;
        self
    }

    /// Called with the index of the radio to select.
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for RadioGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let n = self.options.len();
        let handles = focus::roving_handles(&self.id, n, self.selected, window, cx);
        let radios: Vec<_> = self
            .options
            .into_iter()
            .enumerate()
            .map(|(ix, label)| {
                let f = self.on_select.clone();
                let mut r = Radio::new(child_id(&self.id, ix), ix == self.selected).label(label);
                r.focus = handles.get(ix).cloned();
                if let Some(f) = f {
                    r = r.on_select(move |_, w, cx| f(ix, w, cx));
                }
                r
            })
            .collect();
        let root = div()
            .id(self.id)
            .flex()
            .when(!self.horizontal, |d| d.flex_col().gap(px(SPACE_2)))
            .when(self.horizontal, |d| d.gap(px(SPACE_4)));
        focus::roving_keys(root, handles, self.selected, true, self.on_select).children(radios)
    }
}

/// A small integer input: minus, the value in tabular figures, plus. 32px tall. The buttons
/// dim and stop at `min` and `max` (default 1 to 9); `on_change` receives the new value.
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    value: i32,
    min: i32,
    max: i32,
    on_change: Option<ValueHandler<i32>>,
}

impl Stepper {
    /// A stepper showing `value`, range 1 to 9.
    pub fn new(id: impl Into<ElementId>, value: i32) -> Self {
        Self { id: id.into(), value, min: 1, max: 9, on_change: None }
    }

    /// Set the bounds, inclusive.
    pub fn range(mut self, min: i32, max: i32) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Called with the new value, already within the bounds.
    pub fn on_change(mut self, f: impl Fn(i32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (value, min, max) = (self.value, self.min, self.max);
        let mut button = |suffix: &'static str, glyph: IconName, target: i32, enabled: bool| {
            let id = child_id(&self.id, suffix);
            let selector = id.to_string();
            let faded = motion::hover_fade_state(
                id.clone(),
                Look::bg(TRANSPARENT).fg(TEXT_MUTED),
                if enabled { Look::bg(TRANSPARENT).fg(TEXT) } else { Look::bg(TRANSPARENT).fg(TEXT_MUTED) },
                window,
                cx,
            );
            let color = faded.fg.unwrap_or(TEXT_MUTED);
            let on_change = self.on_change.clone().filter(|_| enabled);
            let el = div()
                .id(id)
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .justify_center()
                .size(px(CONTROL_HEIGHT_MD - 2.))
                .when(!enabled, |d| d.opacity(0.45).cursor_not_allowed())
                .when(enabled, |d| d.cursor_pointer());
            faded
                .apply(el)
                .child(icon(glyph, 12., color))
                .when_some(on_change, |d, f| d.on_click(move |_, w, cx| f(target, w, cx)))
        };
        let dec = button("dec", IconName::Minus, (value - 1).max(min), value > min);
        let inc = button("inc", IconName::Plus, (value + 1).min(max), value < max);
        let typed = window.use_keyed_state(child_id(&self.id, "typed"), cx, |_, _| Typed::default());
        let step = |delta: i32| {
            let f = self.on_change.clone();
            move |window: &mut Window, cx: &mut App| {
                let to = (value + delta).clamp(min, max);
                focus::set_focus_visible(true, window, cx);
                if let Some(f) = f.as_ref().filter(|_| to != value) {
                    f(to, window, cx);
                }
            }
        };
        let (up, down) = (step(1), step(-1));
        let on_digit = self.on_change.clone();
        let selector = self.id.to_string();
        let el = row().id(self.id).debug_selector(move || selector);
        focus::tab_stop(el, false, cx)
            .key_context(crate::keys::STEPPER)
            .on_action(move |_: &MoveUp, w, cx| up(w, cx))
            .on_action(move |_: &MoveDown, w, cx| down(w, cx))
            .on_key_down(move |ev, window, cx| {
                let k = &ev.keystroke;
                let Some(d) = k.key.parse::<u32>().ok().filter(|_| k.key.len() == 1) else { return };
                if k.modifiers.modified() {
                    return;
                }
                let next = typed.update(cx, |t, _| t.push(d, value, min, max));
                cx.stop_propagation();
                if let (Some(to), Some(f)) = (next, on_digit.as_ref()) {
                    f(to, window, cx);
                }
            })
            .flex_none()
            .h(px(CONTROL_HEIGHT_MD))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_MD))
            .bg(SURFACE_RAISED)
            .type_style(BODY)
            .child(dec)
            .child(
                div().min_w(px(20.)).flex().justify_center().tabular().child(value.to_string()),
            )
            .child(inc)
    }
}

/// Digits typed into a focused stepper: a second digit within a second extends the first.
#[derive(Default)]
struct Typed {
    last: Option<(i32, Instant)>,
}

impl Typed {
    /// The value to set after typing `d` on a stepper showing `value`, if any is in range.
    fn push(&mut self, d: u32, value: i32, min: i32, max: i32) -> Option<i32> {
        let d = d as i32;
        let now = Instant::now();
        let longer = self
            .last
            .filter(|(_, at)| now.duration_since(*at) < Duration::from_secs(1))
            .map(|(prev, _)| prev * 10 + d)
            .filter(|v| (min..=max).contains(v));
        let to = longer.or(Some(d).filter(|v| (min..=max).contains(v)));
        self.last = Some((to.unwrap_or(d), now));
        to.filter(|v| *v != value)
    }
}

/// The closed trigger of a select: the value, a chevron and an optional leading icon. The
/// popup that opens from it is the view's job (a menu popover); this is only the box. 32px
/// tall, or `small` 28px.
#[derive(IntoElement)]
pub struct SelectBox {
    id: ElementId,
    value: SharedString,
    icon: Option<IconName>,
    small: bool,
    placeholder: bool,
    open: bool,
    on_click: Option<ClickHandler>,
}

impl SelectBox {
    /// A trigger showing `value`.
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            icon: None,
            small: false,
            placeholder: false,
            open: false,
            on_click: None,
        }
    }

    /// A 14px muted icon before the value.
    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    /// 28px tall, 12px text.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// The value is a placeholder: muted text.
    pub fn placeholder(mut self, placeholder: bool) -> Self {
        self.placeholder = placeholder;
        self
    }

    /// The popup is open: the border shows the accent.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Called when the trigger is clicked.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for SelectBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (h, radius, text, lh) = if self.small {
            (CONTROL_HEIGHT_SM, RADIUS_MD, LABEL, LABEL.line)
        } else {
            (CONTROL_HEIGHT_MD, RADIUS_LG, BODY, BODY.line)
        };
        let border = motion::fade_color(
            child_id(&self.id, "border"),
            if self.open { ACCENT } else { LINE_STRONG },
            window,
            cx,
        );
        let selector = self.id.to_string();
        let el = row().id(self.id).debug_selector(move || selector);
        focus::tab_stop(el, false, cx)
            .gap(px(SPACE_2))
            .h(px(h))
            .pl(px(SPACE_2_5))
            .pr(px(SPACE_2))
            .border_1()
            .border_color(border)
            .rounded(px(radius))
            .bg(SURFACE_RAISED)
            .text_color(if self.placeholder { TEXT_MUTED } else { TEXT })
            .type_style(text)
            .cursor_pointer()
            .when_some(self.icon, |d, i| d.child(icon(i, 14., TEXT_MUTED)))
            .child(truncate_lh(self.value, lh))
            .child(icon(IconName::ChevronDown, 12., TEXT_MUTED))
            .when_some(self.on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}
