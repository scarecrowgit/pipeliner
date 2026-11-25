use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, App, ClickEvent, ElementId, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window,
};
use gpui_component::tooltip::Tooltip;

use super::{icon_sized, kbd, AppIcon};
use crate::theme::{self, TextStyles as _};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Filled accent. One per area at most ("Ship B", "New run").
    Primary,
    /// Raised surface with a border. The default.
    #[default]
    Secondary,
    /// Text only until hovered.
    Ghost,
    /// Red text, soft red hover ("Deny", "Cancel run").
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 28px.
    Small,
    /// 40px (control height).
    #[default]
    Medium,
}

struct Colors {
    bg: Hsla,
    bg_hover: Hsla,
    fg: Hsla,
    fg_hover: Hsla,
    border: Hsla,
}

impl ButtonVariant {
    fn colors(self) -> Colors {
        match self {
            ButtonVariant::Primary => Colors {
                bg: theme::ACCENT_STRONG,
                bg_hover: theme::ACCENT_STRONG_HOVER,
                fg: theme::ON_ACCENT,
                fg_hover: theme::ON_ACCENT,
                border: theme::TRANSPARENT,
            },
            ButtonVariant::Secondary => Colors {
                bg: theme::SURFACE_RAISED,
                bg_hover: theme::SURFACE_HOVER,
                fg: theme::TEXT,
                fg_hover: theme::TEXT,
                border: theme::LINE_STRONG,
            },
            ButtonVariant::Ghost => Colors {
                bg: theme::TRANSPARENT,
                bg_hover: theme::SURFACE_RAISED,
                fg: theme::TEXT_SECONDARY,
                fg_hover: theme::TEXT,
                border: theme::TRANSPARENT,
            },
            ButtonVariant::Danger => Colors {
                bg: theme::TRANSPARENT,
                bg_hover: theme::DANGER_SOFT,
                fg: theme::DANGER,
                fg_hover: theme::DANGER,
                border: theme::TRANSPARENT,
            },
        }
    }
}

/// A button. `Button::new("ship", "Ship B").primary().icon(AppIcon::Check).on_click(...)`.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<AppIcon>,
    icon_right: Option<AppIcon>,
    kbd: Option<SharedString>,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    selected: bool,
    full_width: bool,
    tooltip: Option<SharedString>,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let label: SharedString = label.into();
        Button {
            id: id.into(),
            label: (!label.is_empty()).then_some(label),
            icon: None,
            icon_right: None,
            kbd: None,
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            disabled: false,
            selected: false,
            full_width: false,
            tooltip: None,
            children: Vec::new(),
            on_click: None,
        }
    }
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }
    pub fn secondary(self) -> Self {
        self.variant(ButtonVariant::Secondary)
    }
    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }
    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }
    pub fn small(mut self) -> Self {
        self.size = ButtonSize::Small;
        self
    }
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }
    /// Leading icon.
    pub fn icon(mut self, icon: AppIcon) -> Self {
        self.icon = Some(icon);
        self
    }
    /// Trailing icon (e.g. `ChevronDown` for menus, `ChevronRight` for links).
    pub fn icon_right(mut self, icon: AppIcon) -> Self {
        self.icon_right = Some(icon);
        self
    }
    /// Shortcut hint shown inside the button.
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Pressed / chosen look (segmented controls, "Picked").
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    /// Extra content after the label (a count, a badge).
    pub fn child(mut self, el: impl IntoElement) -> Self {
        self.children.push(el.into_any_element());
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let c = self.variant.colors();
        let small = self.size == ButtonSize::Small;
        let icon_only = self.label.is_none() && self.children.is_empty();
        let pad = match (small, self.variant) {
            (true, _) => px(10.),
            (false, ButtonVariant::Primary) => px(16.),
            (false, ButtonVariant::Secondary) => px(14.),
            (false, _) => px(12.),
        };
        let h = if small {
            theme::CONTROL_H_SM
        } else {
            theme::CONTROL_H
        };
        let icon_px = if small { theme::ICON_SM } else { theme::ICON };
        let (bg, fg, border) = if self.selected {
            (theme::SURFACE_HOVER, theme::TEXT, theme::LINE_STRONG)
        } else {
            (c.bg, c.fg, c.border)
        };
        let disabled = self.disabled;

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(h)
            .when(icon_only, |d| d.w(h))
            .when(!icon_only, |d| d.px(pad))
            .when(self.full_width, |d| d.w_full())
            .rounded(if small {
                theme::RADIUS_MD
            } else {
                theme::RADIUS_LG
            })
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .when(small, |d| d.text_size(px(12.)).line_height(px(16.)))
            .when(!small, |d| d.text_body())
            .font_weight(gpui::FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(disabled, |d| d.opacity(0.45).cursor_not_allowed())
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(c.bg_hover).text_color(c.fg_hover))
                    .active(|s| s.opacity(0.85))
            })
            .when_some(self.icon, |d, i| d.child(icon_sized(i, icon_px)))
            .when_some(self.label, |d, l| d.child(l))
            .children(self.children)
            .when_some(self.kbd, |d, k| d.child(kbd(k)))
            .when_some(self.icon_right, |d, i| {
                d.child(icon_sized(i, icon_px).text_color(theme::TEXT_MUTED))
            })
            .when_some(self.tooltip, |d, t| {
                d.tooltip(move |window, cx| Tooltip::new(t.clone()).build(window, cx))
            })
            .when_some(self.on_click.filter(|_| !disabled), |d, f| {
                d.on_click(move |e, window, cx| {
                    cx.stop_propagation();
                    f(e, window, cx)
                })
            })
    }
}

/// Square icon-only button (ghost by default). Always give it a tooltip.
#[derive(IntoElement)]
pub struct IconButton {
    inner: Button,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: AppIcon) -> Self {
        IconButton {
            inner: Button::new(id, "").icon(icon).ghost(),
        }
    }
    pub fn small(mut self) -> Self {
        self.inner = self.inner.small();
        self
    }
    pub fn variant(mut self, v: ButtonVariant) -> Self {
        self.inner = self.inner.variant(v);
        self
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.inner = self.inner.tooltip(text);
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.inner = self.inner.disabled(d);
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.inner = self.inner.selected(s);
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.inner = self.inner.on_click(f);
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.inner.render(window, cx)
    }
}

/// Segmented control ("7d 30d 90d", "Board | Timeline"): a row of small buttons in a
/// raised track; the selected one is highlighted.
pub fn segmented(
    id: impl Into<SharedString>,
    items: &[&str],
    selected: usize,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id: SharedString = id.into();
    let on_select = Rc::new(on_select);
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(3.))
        .h(theme::CONTROL_H)
        .rounded(theme::RADIUS_LG)
        .border_1()
        .border_color(theme::LINE_STRONG)
        .bg(theme::SURFACE)
        .children(items.iter().enumerate().map(|(ix, label)| {
            let on_select = on_select.clone();
            let active = ix == selected;
            div()
                .id(ElementId::Name(format!("{id}-{ix}").into()))
                .flex()
                .items_center()
                .h_full()
                .px(px(12.))
                .rounded(theme::RADIUS_MD)
                .text_body_strong()
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(theme::SURFACE_HOVER).text_color(theme::TEXT)
                })
                .when(!active, |d| {
                    d.text_color(theme::TEXT_MUTED)
                        .hover(|s| s.text_color(theme::TEXT))
                })
                .child(label.to_string())
                .on_click(move |_, window, cx| on_select(ix, window, cx))
        }))
}
