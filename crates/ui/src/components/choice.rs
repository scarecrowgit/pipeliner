//! Choosing between views and filters: underline tabs, filter chips, segmented controls.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, FontWeight, IntoElement, RenderOnce, SharedString, Window, div,
    prelude::*, px,
};

use super::{ClickHandler, ValueHandler, child_id};
use crate::assets::{IconName, icon};
use crate::motion::{self, Look};
use crate::theme::*;

/// Underline tabs directly under a page header: Flow, Runs, Spawned agents. 44px tall, 20px
/// apart; the selected tab has a 2px accent underline and `text`, the others `text-muted`.
/// Use them to switch views of the same object; 2 to 5 tabs; a count follows the label in
/// muted text.
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    tabs: Vec<(SharedString, Option<SharedString>)>,
    selected: usize,
    flush: bool,
    on_select: Option<ValueHandler<usize>>,
}

impl Tabs {
    /// An empty tab bar.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), tabs: Vec::new(), selected: 0, flush: false, on_select: None }
    }

    /// Add a tab.
    pub fn tab(mut self, label: impl Into<SharedString>) -> Self {
        self.tabs.push((label.into(), None));
        self
    }

    /// Add a tab with a count after its label ("Templates 4").
    pub fn tab_n(mut self, label: impl Into<SharedString>, count: impl Into<SharedString>) -> Self {
        self.tabs.push((label.into(), Some(count.into())));
        self
    }

    /// Index of the selected tab.
    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    /// No inline padding (the bar sits inside a padded container).
    pub fn flush(mut self) -> Self {
        self.flush = true;
        self
    }

    /// Called with the index of the tab that was clicked.
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Tabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tabs: Vec<_> = self
            .tabs
            .into_iter()
            .enumerate()
            .map(|(ix, (label, count))| {
                let selected = ix == self.selected;
                let id = child_id(&self.id, ix);
                let selector = id.to_string();
                let fg = if selected { TEXT } else { TEXT_MUTED };
                let faded = motion::hover_fade_state(
                    id.clone(),
                    Look::bg(TRANSPARENT).fg(fg),
                    Look::bg(TRANSPARENT).fg(TEXT),
                    window,
                    cx,
                );
                let el = div()
                    .id(id)
                    .debug_selector(move || selector)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(SPACE_1_5))
                    .h(px(TAB_HEIGHT))
                    .border_b_2()
                    .border_color(if selected { ACCENT } else { TRANSPARENT })
                    .type_style(if selected { BODY_STRONG } else { BODY })
                    .whitespace_nowrap()
                    .cursor_pointer();
                let on_select = self.on_select.clone();
                faded
                    .apply(el)
                    .child(label)
                    .when_some(count, |d, n| {
                        d.child(
                            div().text_color(TEXT_MUTED).font_weight(FontWeight::NORMAL).tabular().child(n),
                        )
                    })
                    .when_some(on_select, |d, f| d.on_click(move |_, w, cx| f(ix, w, cx)))
            })
            .collect();
        // The bar's hairline sits behind the tabs so the selected underline covers it.
        div()
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(SPACE_5))
            .w_full()
            .h(px(TAB_HEIGHT))
            .when(!self.flush, |d| d.px(px(SPACE_5)))
            .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(LINE))
            .children(tabs)
    }
}

/// A pressable filter chip with an optional count: "Needs you 4". 28px tall; `pressed` is the
/// active filter; `spawn` tints it violet for agent-made items.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    count: Option<SharedString>,
    pressed: bool,
    spawn: bool,
    icon: Option<IconName>,
    on_click: Option<ClickHandler>,
}

impl Chip {
    /// An unpressed chip.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            count: None,
            pressed: false,
            spawn: false,
            icon: None,
            on_click: None,
        }
    }

    /// A count after the label, muted.
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// Whether the filter is on.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Violet text: the chip filters agent-made items.
    pub fn spawn(mut self) -> Self {
        self.spawn = true;
        self
    }

    /// A 12px icon before the label.
    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    /// Called when the chip is clicked.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fg = if self.spawn { SPAWN_TEXT } else { TEXT_SECONDARY };
        let (rest, hover) = if self.pressed {
            let l = Look::bg(SURFACE_HOVER).fg(if self.spawn { SPAWN_TEXT } else { TEXT });
            (l, l)
        } else {
            (
                Look::bg(TRANSPARENT).fg(fg),
                Look::bg(SURFACE_RAISED).fg(if self.spawn { SPAWN_TEXT } else { TEXT }),
            )
        };
        let selector = self.id.to_string();
        let faded = motion::hover_fade_state(self.id.clone(), rest, hover, window, cx);
        let color = faded.fg.unwrap_or(fg);
        let count_color = if self.spawn { SPAWN_TEXT } else { TEXT_MUTED };
        let el = div()
            .id(self.id)
            .debug_selector(move || selector)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(SPACE_1_5))
            .h(px(CONTROL_HEIGHT_SM))
            .px(px(SPACE_2_5))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_MD))
            .type_style(LABEL)
            .whitespace_nowrap()
            .cursor_pointer();
        faded
            .apply(el)
            .when_some(self.icon, |d, i| d.child(icon(i, 12., color)))
            .child(self.label)
            .when_some(self.count, |d, n| d.child(div().text_color(count_color).tabular().child(n)))
            .when_some(self.on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

/// A segmented control: a `surface-raised` track with a `surface-hover` thumb under the
/// selected option. Default 40px, `small` 28px, `full_width` to share the row equally.
/// Inside a panel it replaces underline tabs.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    options: Vec<(Option<IconName>, SharedString)>,
    selected: usize,
    small: bool,
    full: bool,
    on_select: Option<ValueHandler<usize>>,
}

impl Segmented {
    /// A control with no options yet.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: 0,
            small: false,
            full: false,
            on_select: None,
        }
    }

    /// Add an option.
    pub fn option(mut self, label: impl Into<SharedString>) -> Self {
        self.options.push((None, label.into()));
        self
    }

    /// Add an option with a 14px icon before its label.
    pub fn icon_option(mut self, icon: IconName, label: impl Into<SharedString>) -> Self {
        self.options.push((Some(icon), label.into()));
        self
    }

    /// Index of the selected option.
    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    /// 28px tall, 12px text.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// Fill the row; options share the width equally.
    pub fn full_width(mut self) -> Self {
        self.full = true;
        self
    }

    /// Called with the index of the option that was clicked.
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (h, pad, radius, text) = if self.small {
            (CONTROL_HEIGHT_SM, 2., RADIUS_SM, LABEL)
        } else {
            (CONTROL_HEIGHT, 3., RADIUS_MD, BODY)
        };
        let items: Vec<_> = self
            .options
            .into_iter()
            .enumerate()
            .map(|(ix, (ic, label))| {
                let selected = ix == self.selected;
                let id = child_id(&self.id, ix);
                let selector = id.to_string();
                let faded = motion::hover_fade_state(
                    id.clone(),
                    Look::bg(if selected { SURFACE_HOVER } else { TRANSPARENT })
                        .fg(if selected { TEXT } else { TEXT_MUTED }),
                    Look::bg(if selected { SURFACE_HOVER } else { TRANSPARENT }).fg(TEXT),
                    window,
                    cx,
                );
                let color = faded.fg.unwrap_or(TEXT_MUTED);
                let el = div()
                    .id(id)
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(SPACE_1_5))
                    .px(px(if self.small { SPACE_2 } else { SPACE_3 }))
                    .rounded(px(radius))
                    .type_style(if selected { TypeStyle { weight: FontWeight::MEDIUM, ..text } } else { text })
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(self.full, |d| d.flex_1())
                    .when(!self.full, |d| d.flex_none());
                let on_select = self.on_select.clone();
                faded
                    .apply(el)
                    .when_some(ic, |d, i| d.child(icon(i, 14., color)))
                    .child(label)
                    .when_some(on_select, |d, f| d.on_click(move |_, w, cx| f(ix, w, cx)))
            })
            .collect();
        div()
            .id(self.id)
            .flex()
            .gap(px(SPACE_0_5))
            .h(px(h))
            .p(px(pad))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_LG))
            .bg(SURFACE_RAISED)
            .when(self.full, |d| d.w_full())
            .when(!self.full, |d| d.flex_none())
            .children(items)
    }
}
