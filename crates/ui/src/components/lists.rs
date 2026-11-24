//! Rows of lists: sidebar nav items, sidebar list items, popover menu items and their labels.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, HighlightStyle, Hsla, IntoElement, RenderOnce,
    SharedString, Window, div, prelude::*, px,
};

use super::{ClickHandler, CountBadge, Kbd, Status, StatusDot, Tag, child_id};
use crate::assets::{IconName, icon};
use crate::dev::Probe;
use crate::layout::{col, grow, row, truncate_block, truncate_lh, truncate_styled_block};
use crate::tooltip::Tooltip;
use crate::motion::{self, Look};
use crate::theme::*;

/// A row of the sidebar's primary nav: 32px, a 16px icon, the label, and a trailing count or
/// hint. `current` is the open view. Show a count only for things that need action.
#[derive(IntoElement)]
pub struct NavItem {
    id: ElementId,
    icon: IconName,
    label: SharedString,
    count: Option<SharedString>,
    hint: Option<SharedString>,
    current: bool,
    on_click: Option<ClickHandler>,
}

impl NavItem {
    /// A nav row with an icon and a label.
    pub fn new(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            count: None,
            hint: None,
            current: false,
            on_click: None,
        }
    }

    /// An accent count at the right ("4").
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// A warning-coloured hint at the right ("1 waiting").
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The row of the view that is open.
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    /// Called when the row is clicked.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for NavItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (rest, hover) = if self.current {
            let l = Look::bg(SURFACE_HOVER).fg(TEXT);
            (l, l)
        } else {
            (Look::bg(TRANSPARENT).fg(TEXT_SECONDARY), Look::bg(SURFACE_RAISED).fg(TEXT))
        };
        let selector = self.id.to_string();
        let faded = motion::hover_fade_state(self.id.clone(), rest, hover, window, cx);
        let icon_color = if self.current { TEXT } else { TEXT_MUTED };
        let el = row()
            .id(self.id)
            .debug_selector(move || selector)
            .gap(px(SPACE_2_5))
            .h(px(NAV_ITEM_HEIGHT))
            .px(px(SPACE_2))
            .rounded(px(RADIUS_MD))
            .type_style(BODY)
            .cursor_pointer();
        faded
            .apply(el)
            .child(icon(self.icon, 16., icon_color))
            .child(truncate_lh(self.label, BODY.line))
            .when_some(self.hint, |d, h| {
                d.child(div().flex_none().type_style(LABEL).text_color(WARNING).child(h))
            })
            .when_some(self.count, |d, c| d.child(CountBadge::new(c)))
            .when_some(self.on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

/// A 28px sidebar row for a pipeline or a chat: a status dot, the label (ellipsized), and
/// meta (a time, a count). `current` is the open one. `spawned` draws a child row (26px,
/// violet) on a guide line under its parent, for the agents a chat created.
#[derive(IntoElement)]
pub struct ListItem {
    id: ElementId,
    label: SharedString,
    dot: Option<Status>,
    meta: Option<SharedString>,
    current: bool,
    spawned: bool,
    actions: Option<AnyElement>,
    on_click: Option<ClickHandler>,
}

impl ListItem {
    /// A list row labelled `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            dot: None,
            meta: None,
            current: false,
            spawned: false,
            actions: None,
            on_click: None,
        }
    }

    /// Controls shown at the right while the pointer is over the row, in place of the meta
    /// (the sidebar chat's pin and archive buttons: small [`IconButton`](super::IconButton)s).
    /// A click on them does not reach the row's `on_click`.
    pub fn hover_actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }

    /// A status dot before the label.
    pub fn dot(mut self, status: Status) -> Self {
        self.dot = Some(status);
        self
    }

    /// Muted tabular text at the right ("12m", "37 queued").
    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// The row of the chat that is open.
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    /// The child variant, drawn on a violet spawn guide under its parent row.
    pub fn spawned(mut self) -> Self {
        self.spawned = true;
        self
    }

    /// Called when the row is clicked.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for ListItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let spawned = self.spawned;
        let (base, hover_fg) = if spawned { (SPAWN_TEXT, SPAWN_TEXT) } else { (TEXT_SECONDARY, TEXT) };
        let (rest, hover) = if self.current {
            let l = Look::bg(SURFACE_HOVER).fg(if spawned { SPAWN_TEXT } else { TEXT });
            (l, l)
        } else {
            (Look::bg(TRANSPARENT).fg(base), Look::bg(SURFACE_RAISED).fg(hover_fg))
        };
        let selector = self.id.to_string();
        let faded = motion::hover_fade_state(self.id.clone(), rest, hover, window, cx);
        let actions_id = child_id(&self.id, "actions");
        let actions = self.actions.filter(|_| motion::hovered(self.id.clone(), window, cx)).map(|a| {
            // Clicks on the controls stop here, so the row does not open as well.
            div()
                .id(actions_id)
                .flex()
                .flex_none()
                .items_center()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(a)
        });
        let meta = if actions.is_some() { None } else { self.meta };
        let (h, lh, text) = if spawned {
            (LIST_ITEM_CHILD_HEIGHT, LABEL.line, LABEL)
        } else {
            (LIST_ITEM_HEIGHT, BODY.line, BODY)
        };
        let el = row()
            .id(self.id)
            .debug_selector(move || selector)
            .relative()
            .gap(px(SPACE_2))
            .h(px(h))
            .pl(px(if spawned { 30. } else { SPACE_2 }))
            .pr(px(SPACE_2))
            .rounded(px(RADIUS_MD))
            .type_style(text)
            .cursor_pointer();
        faded
            .apply(el)
            .when(spawned, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(SPACE_3))
                        .top(px(-6.))
                        .w(px(10.))
                        .h(px(19.))
                        .border_l_1()
                        .border_b_1()
                        .border_color(SPAWN_LINE)
                        .rounded_bl(px(5.)),
                )
            })
            .when_some(self.dot, |d, s| d.child(div().mx(px(1.)).child(StatusDot::new(s))))
            .child(truncate_lh(self.label, lh))
            .children(actions)
            .when_some(meta, |d, m| {
                d.child(
                    div()
                        .flex_none()
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .tabular()
                        .whitespace_nowrap()
                        .child(m),
                )
            })
            .when_some(self.on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

/// The colour of a [`MenuItem`]'s hint: muted, or one of the status text tones
/// (`.t-info`, `.t-success`…), e.g. "running" in info blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HintTone {
    /// `text-muted`.
    #[default]
    Muted,
    /// `info`: running, live.
    Info,
    /// `success`.
    Success,
    /// `warning`: waiting, needs you.
    Warning,
    /// `danger`: failed.
    Danger,
    /// `spawn-text`: made by an agent.
    Spawn,
    /// `accent-text`.
    Accent,
}

impl HintTone {
    /// The text colour of the tone.
    pub fn color(self) -> Hsla {
        match self {
            HintTone::Muted => TEXT_MUTED,
            HintTone::Info => INFO,
            HintTone::Success => SUCCESS,
            HintTone::Warning => WARNING,
            HintTone::Danger => DANGER,
            HintTone::Spawn => SPAWN_TEXT,
            HintTone::Accent => ACCENT_TEXT,
        }
    }
}

/// One row of a popover or dialog list: an optional check column, an optional icon or tile,
/// a label with an optional muted sub-line, and a hint or key cap on the right. 36px tall at
/// least. `checked` marks the current choice; `selected` the keyboard or pointer position.
/// `disabled` rows are dimmed and skipped; `coming_soon` is the app's way of showing a
/// feature that is not there yet: disabled, with a "Coming soon" tag at the right.
#[derive(IntoElement)]
pub struct MenuItem {
    pub(crate) id: ElementId,
    pub(crate) label: SharedString,
    sub: Option<SharedString>,
    icon: Option<IconName>,
    tile: Option<AnyElement>,
    hint: Option<SharedString>,
    hint_tone: HintTone,
    matched: usize,
    suffix: Option<SharedString>,
    pub(crate) kbd: Option<SharedString>,
    tooltip: Option<SharedString>,
    pub(crate) probe: Option<SharedString>,
    check_column: bool,
    pub(crate) checked: bool,
    pub(crate) selected: bool,
    danger: bool,
    pub(crate) disabled: bool,
    coming_soon: bool,
    pub(crate) on_click: Option<ClickHandler>,
    /// Set by a [`Menu`](crate::menu::Menu): the pointer moved over the row.
    pub(crate) on_move: Option<crate::overlay::Callback>,
}

impl MenuItem {
    /// A menu row labelled `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            sub: None,
            icon: None,
            tile: None,
            hint: None,
            hint_tone: HintTone::Muted,
            matched: 0,
            suffix: None,
            kbd: None,
            tooltip: None,
            probe: None,
            check_column: false,
            checked: false,
            selected: false,
            danger: false,
            disabled: false,
            coming_soon: false,
            on_click: None,
            on_move: None,
        }
    }

    /// A muted line under the label.
    pub fn subline(mut self, sub: impl Into<SharedString>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    /// A 14px icon before the label.
    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    /// A [`Tile`](super::Tile) (`.medium()`, 28px, in a menu) or avatar before the label. The
    /// label of a row with a tile is medium weight.
    pub fn tile(mut self, tile: impl IntoElement) -> Self {
        self.tile = Some(tile.into_any_element());
        self
    }

    /// A muted hint at the right: two or three words.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Small muted mono text right after the label, before the hint: the model of an
    /// agent (`Fixer opus-5.5`). The label gives way first when the row is narrow.
    pub fn label_suffix(mut self, text: impl Into<SharedString>) -> Self {
        self.suffix = Some(text.into());
        self
    }

    /// The colour of the hint (muted by default).
    pub fn hint_tone(mut self, tone: HintTone) -> Self {
        self.hint_tone = tone;
        self
    }

    /// The first `bytes` of the label are what was typed (the matched part of a completion):
    /// drawn in `accent-text`. A length that does not fall on a character is ignored.
    pub fn matched(mut self, bytes: usize) -> Self {
        self.matched = bytes;
        self
    }

    /// A key cap at the right.
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    /// A tooltip on the row; it shows on disabled rows too (why the row is off).
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// The row's dev-driver probe name. Rows in a [`Menu`](crate::menu::Menu) are probed by
    /// their id unless this names them otherwise; disabled rows keep their probe. (Named so,
    /// not `probe`, to leave the [`Probe`] trait's wrapper as it was.)
    pub fn probe_name(mut self, name: impl Into<SharedString>) -> Self {
        self.probe = Some(name.into());
        self
    }

    /// Reserve the 14px check column so rows of a choice list line up.
    pub fn check_column(mut self) -> Self {
        self.check_column = true;
        self
    }

    /// The current choice: shows an accent check (and reserves the column).
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self.check_column = self.check_column || checked;
        self
    }

    /// The keyboard or pointer position: `surface-hover` fill.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// A destructive action: `danger` text.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Dimmed; not clickable, and skipped by the keyboard in a menu.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A feature that is not there yet: disabled, with a "Coming soon" tag at the right.
    pub fn coming_soon(mut self) -> Self {
        self.coming_soon = true;
        self.disabled = true;
        self
    }

    /// Called when the row is clicked.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for MenuItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let disabled = self.disabled;
        let fg: Hsla = match (disabled, self.danger) {
            (true, _) => TEXT_DISABLED,
            (false, true) => DANGER,
            (false, false) => TEXT,
        };
        let (r, h) = motion::row_looks(self.selected && !disabled, SURFACE_HOVER);
        let h = if disabled { r } else { h };
        let selector = self.id.to_string();
        let faded =
            motion::hover_fade_state(self.id.clone(), r.fg(fg), h.fg(fg), window, cx);
        let el = row()
            .id(self.id)
            .debug_selector(move || selector)
            .gap(px(SPACE_2_5))
            .w_full()
            .min_h(px(MENU_ITEM_HEIGHT))
            .px(px(SPACE_2_5))
            .py(px(SPACE_1_5))
            .rounded(px(RADIUS_MD))
            .type_style(BODY)
            .when(!disabled, |d| d.cursor_pointer());
        let sub_color = if disabled { TEXT_DISABLED } else { TEXT_MUTED };
        // Rows that lead with a tile (agents, projects) name things: the label is medium.
        let strong = self.tile.is_some();
        let matched = self.matched.min(self.label.len());
        let marks: Vec<_> = (matched > 0 && self.label.is_char_boundary(matched) && !disabled)
            .then(|| (0..matched, HighlightStyle { color: Some(ACCENT_TEXT), ..HighlightStyle::default() }))
            .into_iter()
            .collect();
        let label = match self.suffix {
            // The label as wide as it is, the suffix right after it.
            Some(suffix) => row()
                .w_full()
                .flex_none()
                .gap(px(SPACE_1_5))
                .h(px(BODY.line))
                .child(crate::truncate::fit_styled(self.label, marks, BODY.line))
                .child(
                    div()
                        .flex_none()
                        .type_style(MONO_SM)
                        .text_color(if disabled { TEXT_DISABLED } else { TEXT_MUTED })
                        .whitespace_nowrap()
                        .child(suffix),
                ),
            None if !marks.is_empty() => truncate_styled_block(self.label, marks, BODY.line),
            None => truncate_block(self.label, BODY.line),
        };
        let hint_color = if disabled { TEXT_DISABLED } else { self.hint_tone.color() };
        let probe = self.probe;
        let el = faded
            .apply(el)
            .when(self.check_column, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .w(px(14.))
                        .when(self.checked, |d| d.child(icon(IconName::Check, 14., ACCENT_TEXT))),
                )
            })
            .when_some(self.tile, |d, t| d.child(t))
            .when_some(self.icon, |d, i| d.child(icon(i, 14., fg)))
            .child(
                col()
                    .flex_grow()
                    .flex_shrink()
                    .min_w_0()
                    .child(label.when(strong, |d| d.font_weight(FontWeight::MEDIUM)))
                    .when_some(self.sub, |d, s| {
                        d.child(truncate_block(s, LABEL.line).type_style(LABEL).text_color(sub_color))
                    }),
            )
            .when_some(self.hint, |d, h| {
                d.child(
                    div()
                        .flex_none()
                        .type_style(LABEL)
                        .text_color(hint_color)
                        .whitespace_nowrap()
                        .child(h),
                )
            })
            .when_some(self.kbd, |d, k| d.child(Kbd::new(k)))
            .when(self.coming_soon, |d| d.child(Tag::new("Coming soon")))
            .when_some(self.tooltip, |d, t| d.tooltip(Tooltip::text(t)))
            .when_some(self.on_move, |d, f| d.on_mouse_move(move |_, w, cx| f(w, cx)))
            .when_some(self.on_click.filter(|_| !disabled), |d, f| {
                d.on_click(move |e, w, cx| f(e, w, cx))
            });
        match probe {
            Some(name) => el.probe(name).into_any_element(),
            None => el.into_any_element(),
        }
    }
}

/// A section label inside a popover: 12px muted, above a group of [`MenuItem`]s.
#[derive(IntoElement)]
pub struct PopLabel {
    text: SharedString,
    trailing: Option<AnyElement>,
}

impl PopLabel {
    /// A label reading `text`.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), trailing: None }
    }

    /// Something at the right edge (a count, a link).
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing = Some(el.into_any_element());
        self
    }
}

impl RenderOnce for PopLabel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        row()
            .gap(px(SPACE_1_5))
            .px(px(SPACE_2_5))
            .pt(px(SPACE_2))
            .pb(px(SPACE_1))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(self.text)
            .when_some(self.trailing, |d, t| d.child(grow()).child(t))
    }
}

/// A `line-strong` rule between groups of a popover, reaching the popover's edges (the
/// popover pads 4px).
#[derive(IntoElement)]
pub struct PopSeparator;

impl RenderOnce for PopSeparator {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().h(px(1.)).flex_none().my(px(SPACE_1)).mx(px(-SPACE_1)).bg(LINE_STRONG)
    }
}
