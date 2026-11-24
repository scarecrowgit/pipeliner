//! The inspector's building blocks: panel sections, labelled fields, stepper rows, pickers
//! that open a menu through the overlay host, and tags that can be removed.

use std::rc::Rc;

use gpui::{AnyElement, App, ClickEvent, Context, Div, SharedString, Window, div, prelude::*, px};
use hub::model::NodeKind;
use ui::dev::Probe;
use ui::motion::Look;
use ui::prelude::*;

use super::InspectorView;

/// The width inside a section: the panel less its 20px padding on each side.
pub(crate) const INNER: f32 = PANEL_WIDTH - 2. * SPACE_5;

/// A panel section without a title (`.psec`): 16/20 padding and a hairline below.
pub(crate) fn psec() -> Div {
    col().flex_none().px(px(SPACE_5)).py(px(SPACE_4)).border_b_1().border_color(LINE)
}

/// A panel section with a title and optional meta at the right of its head.
pub(crate) fn titled(title: impl Into<SharedString>, meta: Option<AnyElement>) -> Div {
    let head = row()
        .gap(px(SPACE_2))
        .min_h(px(20.))
        .mb(px(SPACE_2_5))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(div().type_style(BODY_STRONG).text_color(TEXT).child(title.into()))
        .child(grow())
        .children(meta);
    psec().child(head)
}

/// A label above a control, 6px apart (`.field`).
pub(crate) fn field(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    col()
        .gap(px(SPACE_1_5))
        .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(label.into()))
        .child(control)
}

/// A muted label on the left and a compact control on the right (a stepper, a switch).
pub(crate) fn row_field(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    row()
        .gap(px(SPACE_3))
        .min_h(px(CONTROL_HEIGHT_MD))
        .child(div().flex_1().min_w_0().type_style(LABEL).text_color(TEXT_MUTED).child(label.into()))
        .child(control)
}

/// Explanatory text at a definite width, so it wraps (`.note`).
pub(crate) fn note(text: impl Into<SharedString>, width: f32) -> Div {
    div().w(px(width)).type_style(LABEL).text_color(TEXT_MUTED).child(text.into())
}


/// The tile icon of a node kind, as the canvas draws it.
pub(crate) fn kind_icon(kind: &NodeKind) -> IconName {
    match kind {
        NodeKind::Source(_) => IconName::Source,
        NodeKind::Router(_) => IconName::Router,
        NodeKind::Agent(_) => IconName::Agent,
        NodeKind::Orchestrator(_) => IconName::Orchestrator,
        NodeKind::Subagent(_) => IconName::Spawn,
        NodeKind::File(_) => IconName::File,
        NodeKind::Review(_) => IconName::Inbox,
        NodeKind::Action(_) => IconName::Zap,
        NodeKind::Pipeline(_) => IconName::Pipelines,
    }
}

/// The tile tone of a node kind: violet only for what an agent creates.
pub(crate) fn kind_tone(kind: &NodeKind) -> TileTone {
    match kind {
        NodeKind::Source(_) => TileTone::Info,
        NodeKind::Agent(_) | NodeKind::Orchestrator(_) => TileTone::Accent,
        NodeKind::Subagent(_) => TileTone::Spawn,
        _ => TileTone::Neutral,
    }
}

/// One row of a picker's menu.
#[derive(Clone, Default)]
pub(crate) struct Pick {
    pub label: SharedString,
    pub sub: Option<SharedString>,
    pub hint: Option<SharedString>,
    pub icon: Option<IconName>,
    pub checked: bool,
    /// Drawn disabled with "Coming soon".
    pub soon: bool,
    /// A group heading drawn above this row.
    pub group: Option<SharedString>,
}

impl Pick {
    pub(crate) fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), ..Self::default() }
    }

    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub(crate) fn sub(mut self, sub: impl Into<SharedString>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    pub(crate) fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub(crate) fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub(crate) fn soon(mut self) -> Self {
        self.soon = true;
        self
    }

    pub(crate) fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// What a picker does with the chosen row.
pub(crate) type OnPick = Rc<dyn Fn(&mut InspectorView, usize, &mut Window, &mut Context<InspectorView>)>;

impl InspectorView {
    /// A danger line under a control that a problem is about, `width` wide; it can be
    /// selected and copied.
    pub(crate) fn problem_line(&mut self, problem: Option<SharedString>, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let message = problem?;
        let text = self.texts.text("problem-line", message, LABEL, DANGER, cx);
        Some(div().w(px(width)).child(text).into_any_element())
    }

    /// A select box showing `value` that opens a menu of `picks`; rows are probed as
    /// `<key>-<index>`. `muted` draws the value as a placeholder (an inherited value).
    #[expect(clippy::too_many_arguments, reason = "a picker is its value, its rows and what a pick does")]
    pub(crate) fn picker(
        &self,
        key: impl Into<SharedString>,
        value: impl Into<SharedString>,
        muted: bool,
        lead: Option<IconName>,
        picks: Vec<Pick>,
        on_pick: OnPick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key: SharedString = key.into();
        let open = ui::overlays(window, cx).is_open(key.clone(), cx);
        let view = cx.entity().downgrade();
        let picks = Rc::new(picks);
        let k = key.clone();
        let mut select = SelectBox::new(key.clone(), value).placeholder(muted).open(open).on_click(move |_, window, cx| {
            let anchor = Anchor::of(k.clone(), window, cx);
            let (picks, on_pick, view, k) = (picks.clone(), on_pick.clone(), view.clone(), k.clone());
            ui::overlays(window, cx).open_menu(
                anchor,
                Placement::below(),
                move |_, _| {
                    let mut menu = Menu::new(SharedString::from(format!("{k}-menu"))).width(INNER).max_height(420.);
                    for (ix, pick) in picks.iter().enumerate() {
                        if let Some(group) = &pick.group {
                            if ix > 0 {
                                menu = menu.separator();
                            }
                            menu = menu.label(group.clone());
                        }
                        let mut item = MenuItem::new(SharedString::from(format!("{k}-{ix}")), pick.label.clone())
                            .check_column()
                            .checked(pick.checked);
                        if let Some(sub) = &pick.sub {
                            item = item.subline(sub.clone());
                        }
                        if let Some(hint) = &pick.hint {
                            item = item.hint(hint.clone());
                        }
                        if let Some(icon) = pick.icon {
                            item = item.icon(icon);
                        }
                        if pick.soon {
                            item = item.coming_soon();
                        } else {
                            let (view, on_pick) = (view.clone(), on_pick.clone());
                            item = item.on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| on_pick(this, ix, window, cx)).ok();
                            });
                        }
                        menu = menu.item(item);
                    }
                    menu
                },
                window,
                cx,
            );
        });
        if let Some(lead) = lead {
            select = select.icon(lead);
        }
        div().w_full().min_w_0().child(select.popover_anchor(key.clone())).probe(key).into_any_element()
    }

    /// A value callback for a `ui` control that edits through this view.
    pub(crate) fn on_value<T: 'static>(
        &self,
        cx: &Context<Self>,
        f: impl Fn(&mut Self, T, &mut Context<Self>) + 'static,
    ) -> impl Fn(T, &mut Window, &mut App) + 'static {
        let view = cx.entity().downgrade();
        move |value, _, cx| {
            view.update(cx, |this, cx| f(this, value, cx)).ok();
        }
    }

    /// A click callback that edits through this view.
    pub(crate) fn on_click(
        &self,
        cx: &Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let view = cx.entity().downgrade();
        move |_, window, cx| {
            view.update(cx, |this, cx| f(this, window, cx)).ok();
        }
    }
}

/// A mono tag with an × that removes it (`.tag.mono`); `id` is the ×'s id and probe. A text
/// too long for [`INNER`] ends in `…`.
pub(crate) fn removable_tag(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    tone: TagTone,
    on_remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id: SharedString = id.into();
    let text: SharedString = text.into();
    let (bg, fg) = match tone {
        TagTone::Neutral => (SURFACE_HOVER, TEXT_SECONDARY),
        TagTone::Spawn => (SPAWN_SOFT, SPAWN_TEXT),
        TagTone::Accent => (ACCENT_SOFT, ACCENT_TEXT),
    };
    let fade = ui::motion::hover_fade_state(id.clone(), Look::bg(TRANSPARENT).fg(TEXT_MUTED), Look::bg(TRANSPARENT).fg(TEXT), window, cx);
    let x_color = fade.fg.unwrap_or(TEXT_MUTED);
    let selector = id.to_string();
    let x = fade
        .apply(div().id(id.clone()).debug_selector(move || selector))
        .flex()
        .items_center()
        .justify_center()
        .size(px(14.))
        .rounded(px(RADIUS_SM))
        .cursor_pointer()
        .child(icon(IconName::X, 10., x_color))
        .on_click(on_remove);
    row()
        .flex_none()
        .max_w(px(INNER))
        .gap(px(SPACE_1))
        .h(px(20.))
        .pl(px(SPACE_1_5))
        .pr(px(SPACE_0_5))
        .rounded(px(RADIUS_MD))
        .bg(bg)
        .text_color(fg)
        .type_style(MONO_SM)
        .whitespace_nowrap()
        .child(div().flex_shrink().min_w_0().child(truncate_lh(text, MONO_SM.line)))
        .child(div().flex_none().child(x.probe(id)))
        .into_any_element()
}

/// Tags that wrap at a definite width.
pub(crate) fn tag_row(width: f32, tags: impl IntoIterator<Item = AnyElement>) -> Div {
    div().w(px(width)).flex().flex_row().flex_wrap().gap(px(SPACE_1_5)).children(tags)
}
