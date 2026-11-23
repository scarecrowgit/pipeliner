//! Actions: buttons, icon buttons and links.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, Hsla, IntoElement, RenderOnce, SharedString, Window, div,
    prelude::*, px,
};

use super::{ClickHandler, Kbd, child_id};
use crate::focus;
use crate::tooltip::Tooltip;
use crate::assets::{IconName, icon};
use crate::motion::{self, Look};
use crate::theme::*;

/// The weight of a [`Button`]: how loud the action is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum ButtonKind {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
}

/// An action someone takes. Variants: `primary` (at most one per view), `secondary` (the
/// default), `ghost` for low-stakes toolbar actions, `danger` text for destructive ones; 40px
/// tall or `small` (28px); with a leading icon and a trailing `kbd` hint; `disabled`;
/// `selected` for a pressed toggle. Labels start with a verb.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    kind: ButtonKind,
    small: bool,
    icon: Option<IconName>,
    kbd: Option<SharedString>,
    disabled: bool,
    selected: bool,
    full_width: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    /// A secondary button labelled `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ButtonKind::Secondary,
            small: false,
            icon: None,
            kbd: None,
            disabled: false,
            selected: false,
            full_width: false,
            on_click: None,
        }
    }

    /// The one action the view exists for: `accent-strong` fill.
    pub fn primary(mut self) -> Self {
        self.kind = ButtonKind::Primary;
        self
    }

    /// The default look: `surface-raised` fill with a `line-strong` border.
    pub fn secondary(mut self) -> Self {
        self.kind = ButtonKind::Secondary;
        self
    }

    /// No fill until hovered.
    pub fn ghost(mut self) -> Self {
        self.kind = ButtonKind::Ghost;
        self
    }

    /// Red text, no fill: reject, cancel run.
    pub fn danger(mut self) -> Self {
        self.kind = ButtonKind::Danger;
        self
    }

    /// 28px tall, 12px text.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// A leading icon (14px, 12px when small).
    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    /// A trailing key cap hint such as "⌘↵".
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    /// Dimmed, no hover, no click.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The pressed look of a toggle button: `surface-hover` fill.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Fill the row's width.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// Called when the button is clicked (never when disabled).
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let sm = self.small;
        let (pad, radius, icon_size, gap) = match (self.kind, sm) {
            (ButtonKind::Primary, false) => (SPACE_4, RADIUS_LG, 14., SPACE_1_5),
            (ButtonKind::Ghost | ButtonKind::Danger, false) => (SPACE_3, RADIUS_LG, 14., SPACE_1_5),
            (_, false) => (SPACE_3_5, RADIUS_LG, 14., SPACE_1_5),
            (ButtonKind::Ghost | ButtonKind::Danger, true) => (SPACE_2, RADIUS_MD, 12., 5.),
            (_, true) => (SPACE_2_5, RADIUS_MD, 12., 5.),
        };
        let (bg, hover_bg, border, fg, hover_fg) = match self.kind {
            ButtonKind::Primary => (ACCENT_STRONG, ACCENT_STRONG_HOVER, TRANSPARENT, ON_ACCENT, ON_ACCENT),
            ButtonKind::Secondary => (SURFACE_RAISED, SURFACE_HOVER, LINE_STRONG, TEXT, TEXT),
            ButtonKind::Ghost => (TRANSPARENT, SURFACE_RAISED, TRANSPARENT, TEXT_SECONDARY, TEXT),
            ButtonKind::Danger => (TRANSPARENT, DANGER_SOFT, TRANSPARENT, DANGER, DANGER),
        };
        let pressed = self.selected && self.kind != ButtonKind::Primary;
        let (bg, border, fg) =
            if pressed { (SURFACE_HOVER, LINE_STRONG, TEXT) } else { (bg, border, fg) };
        let rest = Look::bg(bg).fg(fg).border(border);
        let hover = if self.disabled || pressed {
            rest
        } else {
            Look::bg(hover_bg).fg(hover_fg).border(border)
        };

        let selector = self.id.to_string();
        let faded = motion::hover_fade_state(self.id.clone(), rest, hover, window, cx);
        let icon_color = faded.fg.unwrap_or(fg);
        let disabled = self.disabled;
        let on_click = self.on_click.filter(|_| !disabled);
        let el = div().id(self.id).debug_selector(move || selector);
        let el = focus::tab_stop(el, disabled, cx)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(gap))
            .h(px(if sm { CONTROL_HEIGHT_SM } else { CONTROL_HEIGHT }))
            .px(px(pad))
            .when(self.full_width, |d| d.w_full())
            .rounded(px(radius))
            .border_1()
            .type_style(if sm { LABEL_STRONG } else { BODY_STRONG })
            .whitespace_nowrap()
            .when(disabled, |d| d.opacity(0.45).cursor_not_allowed())
            .when(!disabled, |d| d.cursor_pointer());
        faded
            .apply(el)
            .when_some(self.icon, |d, name| d.child(icon(name, icon_size, icon_color)))
            .child(self.label)
            .when_some(self.kbd, |d, keys| {
                let kbd = Kbd::new(keys);
                let kbd = if self.kind == ButtonKind::Primary { kbd.on_accent() } else { kbd };
                d.child(div().ml(px(SPACE_0_5)).child(kbd))
            })
            .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

/// An icon-only button, 32px square. Variants: plain (muted, fills on hover), `bordered`
/// (`surface-raised` with a `line-strong` border), `small` (28px), `rail` (36px in the
/// collapsed sidebar, with an optional `pip` dot or `count`), `current` for the open view.
/// Always give it a `tooltip`: it is the control's only label.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    bordered: bool,
    small: bool,
    rail: bool,
    current: bool,
    disabled: bool,
    tooltip: Option<SharedString>,
    pip: Option<Hsla>,
    count: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    /// A plain 32px icon button.
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            bordered: false,
            small: false,
            rail: false,
            current: false,
            disabled: false,
            tooltip: None,
            pip: None,
            count: None,
            on_click: None,
        }
    }

    /// Border and raised fill (`.ibtn.bd`).
    pub fn bordered(mut self) -> Self {
        self.bordered = true;
        self
    }

    /// 28px square.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// 36px square with an 18px icon: the collapsed sidebar rail.
    pub fn rail(mut self) -> Self {
        self.rail = true;
        self
    }

    /// The button of the view that is open: `surface-hover` fill, `text` colour.
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    /// Dimmed, no hover, no click.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The label shown on hover.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// A status dot in the top-right corner (rail buttons).
    pub fn pip(mut self, color: Hsla) -> Self {
        self.pip = Some(color);
        self
    }

    /// A count pill in the top-right corner (rail buttons).
    pub fn count(mut self, text: impl Into<SharedString>) -> Self {
        self.count = Some(text.into());
        self
    }

    /// Called when the button is clicked (never when disabled).
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = if self.rail {
            RAIL_BUTTON
        } else if self.small {
            CONTROL_HEIGHT_SM
        } else {
            CONTROL_HEIGHT_MD
        };
        let icon_size = if self.rail { 18. } else { 16. };
        let (rest, hover) = if self.current {
            let l = Look::bg(SURFACE_HOVER).fg(TEXT).border(TRANSPARENT);
            (l, l)
        } else if self.bordered {
            (
                Look::bg(SURFACE_RAISED).fg(TEXT_SECONDARY).border(LINE_STRONG),
                Look::bg(SURFACE_HOVER).fg(TEXT).border(LINE_STRONG),
            )
        } else {
            (
                Look::bg(TRANSPARENT).fg(TEXT_MUTED).border(TRANSPARENT),
                Look::bg(SURFACE_RAISED).fg(TEXT).border(TRANSPARENT),
            )
        };
        let hover = if self.disabled { rest } else { hover };
        let selector = self.id.to_string();
        let faded = motion::hover_fade_state(self.id.clone(), rest, hover, window, cx);
        let color = faded.fg.unwrap_or(TEXT_MUTED);
        let disabled = self.disabled;
        let on_click = self.on_click.filter(|_| !disabled);
        let el = div().id(self.id).debug_selector(move || selector);
        let el = focus::tab_stop(el, disabled, cx)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(size))
            .rounded(px(RADIUS_MD))
            .border_1()
            .when(disabled, |d| d.opacity(0.45).cursor_not_allowed())
            .when(!disabled, |d| d.cursor_pointer());
        faded
            .apply(el)
            .child(icon(self.icon, icon_size, color))
            .when_some(self.pip, |d, c| {
                d.child(
                    div()
                        .absolute()
                        .top(px(5.))
                        .right(px(5.))
                        .size(px(8.))
                        .rounded_full()
                        .border_2()
                        .border_color(BG_SIDEBAR)
                        .bg(c),
                )
            })
            .when_some(self.count, |d, n| {
                d.child(
                    div()
                        .absolute()
                        .top(px(SPACE_0_5))
                        .right_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w(px(16.))
                        .h(px(16.))
                        .px(px(SPACE_1))
                        .rounded_full()
                        .bg(ACCENT_STRONG)
                        .text_color(ON_ACCENT)
                        .type_style(MICRO)
                        .child(n),
                )
            })
            .when_some(self.tooltip, |d, t| d.tooltip(Tooltip::text(t)))
            .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

/// Inline text that goes somewhere: `accent-text`, underlined on hover.
#[derive(IntoElement)]
pub struct Link {
    id: ElementId,
    text: SharedString,
    on_click: Option<ClickHandler>,
}

impl Link {
    /// A link reading `text`.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self { id: id.into(), text: text.into(), on_click: None }
    }

    /// Called when the link is clicked.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Link {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let selector = self.id.to_string();
        let look = Look::bg(TRANSPARENT).fg(ACCENT_TEXT);
        let faded = motion::hover_fade_state(child_id(&self.id, "hover"), look, look, window, cx);
        let underline = motion::hovered(child_id(&self.id, "hover"), window, cx);
        let el = div().id(self.id).debug_selector(move || selector);
        let el = focus::tab_stop(el, false, cx)
            .flex_none()
            .rounded(px(RADIUS_SM))
            .cursor_pointer()
            .when(underline, |d| d.underline());
        faded
            .apply(el)
            .child(self.text)
            .when_some(self.on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}
