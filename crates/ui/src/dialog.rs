//! Dialogs: the centred panel over the scrim, and the pieces the mockups' dialogs are made
//! of: radio option cards and the type-to-confirm field.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, FocusHandle, IntoElement, ParentElement, RenderOnce, SharedString,
    Window, div, prelude::*, px,
};

use crate::assets::IconName;
use crate::components::{
    Button, ClickHandler, IconButton, InputFrame, ValueHandler, child_id, radio_dot,
};
use crate::focus;
use crate::keys::{self, Cancel, Confirm};
use crate::layout::{col, grow, row};
use crate::motion::{self, Look};
use crate::overlay::{Callback, LayerId, current_layer, overlays};
use crate::theme::*;

/// A dialog panel: a head (title 15/22 medium, an optional muted sub-line, a close button),
/// a body that scrolls (blocks 18px apart, 20px padding), and a foot on the sidebar ground
/// with a note on the left and buttons on the right. Fixed width (640 by default) and a
/// fixed distance from the top (12% of the window by default).
///
/// Escape, the close button and [`Dialog::cancel_button`] cancel (`on_cancel`, then close);
/// Cmd/Ctrl-Enter runs the primary action. Open it with
/// [`Overlays::open_dialog`](crate::overlay::Overlays::open_dialog); focus moves to its first
/// control (in the body, else the close button) and the host keeps Tab inside it.
#[derive(IntoElement)]
pub struct Dialog {
    id: ElementId,
    title: SharedString,
    sub: Option<AnyElement>,
    width: f32,
    top: Option<f32>,
    close_button: bool,
    body: Vec<AnyElement>,
    note: Option<AnyElement>,
    actions: Vec<Foot>,
    on_confirm: Option<Callback>,
    on_cancel: Option<Callback>,
    layer: Option<LayerId>,
}

impl Dialog {
    /// A dialog titled `title`. `id` keeps its focus between frames.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Dialog {
            id: id.into(),
            title: title.into(),
            sub: None,
            width: DIALOG_WIDTH,
            top: None,
            close_button: true,
            body: Vec::new(),
            note: None,
            actions: Vec::new(),
            on_confirm: None,
            on_cancel: None,
            layer: current_layer(),
        }
    }

    /// A muted line under the title (text or an element with mono parts).
    pub fn subtitle(mut self, sub: impl IntoElement) -> Self {
        self.sub = Some(sub.into_any_element());
        self
    }

    /// Width in px.
    pub fn width(mut self, px: f32) -> Self {
        self.width = px;
        self
    }

    /// Distance from the window's top in px.
    pub fn top(mut self, px: f32) -> Self {
        self.top = Some(px);
        self
    }

    /// Leave out the close button in the head.
    pub fn no_close_button(mut self) -> Self {
        self.close_button = false;
        self
    }

    /// Muted text (or any element) at the left of the foot.
    pub fn note(mut self, note: impl IntoElement) -> Self {
        self.note = Some(note.into_any_element());
        self
    }

    /// A button (or anything) at the right of the foot, after those already added.
    pub fn action(mut self, el: impl IntoElement) -> Self {
        self.actions.push(Foot::El(el.into_any_element()));
        self
    }

    /// A ghost "Cancel" button with an Esc key cap that cancels.
    pub fn cancel_button(mut self) -> Self {
        self.actions.push(Foot::Cancel);
        self
    }

    /// The primary action: `button` (style it `.primary().kbd("⌘↵")`) and Cmd/Ctrl-Enter call
    /// `f`, then the dialog closes.
    pub fn primary_action(mut self, button: Button, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        let layer = self.layer;
        let run: Callback = Rc::new(move |window, cx| {
            f(window, cx);
            if let Some(layer) = layer {
                overlays(window, cx).close_layer(layer, window, cx);
            }
        });
        let click = run.clone();
        self.actions.push(Foot::El(button.on_click(move |_, w, cx| click(w, cx)).into_any_element()));
        self.on_confirm = Some(run);
        self
    }

    /// What Cmd/Ctrl-Enter does, without closing (for a confirm that can fail validation).
    pub fn on_confirm(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(f));
        self
    }

    /// Called when the dialog is cancelled (Escape, the close button, Cancel), before it
    /// closes. A press on the scrim closes it without this.
    pub fn on_cancel(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(f));
        self
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

/// An entry of the foot's button row. The Cancel button is made when the dialog renders,
/// since it needs the dialog's cancel callback.
enum Foot {
    El(AnyElement),
    Cancel,
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let vh = f32::from(window.viewport_size().height);
        let top = self.top.unwrap_or((vh * DIALOG_TOP).round());
        let max_h = (vh - top - SPACE_6).max(CONTROL_HEIGHT * 4.);
        let state = window.use_keyed_state(child_id(&self.id, "focus"), cx, |window, cx| {
            let (root, body) = (cx.focus_handle(), cx.focus_handle());
            window.focus(&root);
            // Once this frame is drawn the controls are in the tab order: focus the first.
            let (r, b) = (root.clone(), body.clone());
            window.defer(cx, move |window, cx| focus_first_control(&r, &b, window, cx));
            DialogFocus { root, body }
        });
        let (focus, body_focus) = {
            let s = state.read(cx);
            (s.root.clone(), s.body.clone())
        };

        let (layer, on_cancel) = (self.layer, self.on_cancel.clone());
        let cancel: Callback = Rc::new(move |window, cx| {
            if let Some(f) = &on_cancel {
                f(window, cx);
            }
            if let Some(layer) = layer {
                overlays(window, cx).close_layer(layer, window, cx);
            }
        });

        let head = row()
            .items_start()
            .gap(px(SPACE_3))
            .pt(px(SPACE_4))
            .pr(px(SPACE_3))
            .pb(px(SPACE_3_5))
            .pl(px(SPACE_5))
            .border_b_1()
            .border_color(LINE)
            .flex_none()
            .child(
                col()
                    .flex_1()
                    .child(div().type_style(TITLE_SM).child(self.title))
                    .when_some(self.sub, |d, s| {
                        d.child(div().mt(px(SPACE_0_5)).type_style(LABEL).text_color(TEXT_MUTED).child(s))
                    }),
            )
            .when(self.close_button, |d| {
                let cancel = cancel.clone();
                d.child(
                    IconButton::new(child_id(&self.id, "close"), IconName::X)
                        .small()
                        .tooltip("Close")
                        .on_click(move |_, w, cx| cancel(w, cx)),
                )
            });

        let body = div()
            .id(child_id(&self.id, "body"))
            .track_focus(&body_focus)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(px(DIALOG_GAP))
            .p(px(SPACE_5))
            .children(self.body.into_iter().map(|c| div().flex_none().child(c)));

        let has_foot = self.note.is_some() || !self.actions.is_empty();
        let actions: Vec<AnyElement> = self
            .actions
            .into_iter()
            .enumerate()
            .map(|(ix, a)| match a {
                Foot::El(el) => el,
                Foot::Cancel => {
                    let cancel = cancel.clone();
                    Button::new(child_id(&self.id, format!("cancel-{ix}")), "Cancel")
                        .ghost()
                        .kbd("Esc")
                        .on_click(move |_, w, cx| cancel(w, cx))
                        .into_any_element()
                }
            })
            .collect();
        let foot = has_foot.then(|| {
            row()
                .flex_none()
                .gap(px(SPACE_2))
                .pt(px(SPACE_3))
                .pr(px(SPACE_4))
                .pb(px(SPACE_3))
                .pl(px(SPACE_5))
                .border_t_1()
                .border_color(LINE)
                .bg(BG_SIDEBAR)
                .when_some(self.note, |d, n| {
                    d.child(row().gap(px(SPACE_1_5)).type_style(LABEL).text_color(TEXT_MUTED).child(n))
                })
                .child(grow())
                .children(actions)
        });

        let panel = col()
            .w(px(self.width))
            .max_h(px(max_h))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_DIALOG))
            .bg(SURFACE)
            .overflow_hidden()
            .occlude()
            .child(head)
            .child(body)
            .children(foot);

        let confirm = self.on_confirm;
        let selector = self.id.to_string();
        div()
            .id(self.id)
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .pt(px(top))
            .track_focus(&focus)
            .key_context(keys::DIALOG)
            .on_action(move |_: &Cancel, w, cx| cancel(w, cx))
            .on_action(move |_: &Confirm, w, cx| {
                if let Some(f) = &confirm {
                    f(w, cx);
                }
            })
            .child(panel)
    }
}

/// Focus handles of an open dialog: its root (key context) and its body.
struct DialogFocus {
    root: FocusHandle,
    body: FocusHandle,
}

/// Focus the dialog's first control (the body's first, else the head's close button), unless
/// focus has left the dialog meanwhile.
fn focus_first_control(root: &FocusHandle, body: &FocusHandle, window: &mut Window, cx: &mut App) {
    if !root.is_focused(window) {
        return;
    }
    let inside = |h: &FocusHandle, window: &Window, cx: &App| {
        window.focused(cx).is_some_and(|f| &f != h && h.contains(&f, window))
    };
    for scope in [body, root] {
        window.focus(scope);
        window.focus_next();
        if inside(scope, window, cx) {
            return;
        }
    }
    window.focus(root);
}

/// A radio card (`.opt`): a radio, a strong title, a muted description and optional content
/// at the right. The selected card has an `accent-ring` border on `surface-raised`. Put them
/// in an [`OptionGroup`] so arrow keys move between them.
#[derive(IntoElement)]
pub struct OptionCard {
    id: ElementId,
    title: SharedString,
    desc: Option<SharedString>,
    right: Option<AnyElement>,
    selected: bool,
    on_select: Option<ClickHandler>,
    focus: Option<FocusHandle>,
    probe: Option<SharedString>,
}

impl OptionCard {
    /// A card titled `title`.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        OptionCard {
            id: id.into(),
            title: title.into(),
            desc: None,
            right: None,
            selected: false,
            on_select: None,
            focus: None,
            probe: None,
        }
    }

    /// The card's dev-driver probe name; it passes through an [`OptionGroup`].
    pub fn probe_name(mut self, name: impl Into<SharedString>) -> Self {
        self.probe = Some(name.into());
        self
    }

    /// The muted line under the title.
    pub fn description(mut self, desc: impl Into<SharedString>) -> Self {
        self.desc = Some(desc.into());
        self
    }

    /// Something at the right (a badge: "your pick").
    pub fn right(mut self, el: impl IntoElement) -> Self {
        self.right = Some(el.into_any_element());
        self
    }

    /// The chosen card.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Called when the card is pressed while not selected.
    pub fn on_select(mut self, f: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for OptionCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let selected = self.selected;
        let (rest, hover) = if selected {
            let l = Look::bg(SURFACE_RAISED).border(ACCENT_RING);
            (l, l)
        } else {
            (Look::bg(SURFACE).border(LINE), Look::bg(SURFACE_RAISED).border(LINE))
        };
        let faded = motion::hover_fade_state(self.id.clone(), rest, hover, window, cx);
        let dot = radio_dot(&self.id, selected, window, cx);
        let selector = self.id.to_string();
        let el = div().id(self.id).debug_selector(move || selector);
        let el = match &self.focus {
            Some(h) => focus::track(el, h, cx),
            None => focus::tab_stop(el, false, cx),
        };
        let el = el
            .flex()
            .items_start()
            .gap(px(SPACE_2_5))
            .w_full()
            .py(px(SPACE_2_5))
            .px(px(SPACE_3))
            .border_1()
            .rounded(px(RADIUS_LG))
            .cursor_pointer();
        let probe = self.probe;
        let el = faded
            .apply(el)
            .child(div().mt(px(SPACE_0_5)).child(dot))
            .child(
                col()
                    .flex_1()
                    .child(div().type_style(BODY_STRONG).child(self.title))
                    .when_some(self.desc, |d, s| d.child(div().type_style(LABEL).text_color(TEXT_MUTED).child(s))),
            )
            .when_some(self.right, |d, r| d.child(div().flex_none().child(r)))
            .when_some(self.on_select.filter(|_| !selected), |d, f| d.on_click(move |e, w, cx| f(e, w, cx)));
        match probe {
            Some(name) => crate::dev::Probe::probe(el, name).into_any_element(),
            None => el.into_any_element(),
        }
    }
}

/// [`OptionCard`]s that belong together (`.opts`, 6px apart): one tab stop, arrow keys move
/// the selection, `on_select` gets the index of the card to select.
#[derive(IntoElement)]
pub struct OptionGroup {
    id: ElementId,
    cards: Vec<OptionCard>,
    selected: usize,
    on_select: Option<ValueHandler<usize>>,
}

impl OptionGroup {
    /// An empty group.
    pub fn new(id: impl Into<ElementId>) -> Self {
        OptionGroup { id: id.into(), cards: Vec::new(), selected: 0, on_select: None }
    }

    /// Add a card (its own `selected` and `on_select` are replaced by the group's).
    pub fn option(mut self, card: OptionCard) -> Self {
        self.cards.push(card);
        self
    }

    /// Index of the selected card.
    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    /// Called with the index of the card to select.
    pub fn on_select(mut self, f: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for OptionGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handles = focus::roving_handles(&self.id, self.cards.len(), self.selected, window, cx);
        let cards: Vec<OptionCard> = self
            .cards
            .into_iter()
            .enumerate()
            .map(|(ix, mut card)| {
                card.selected = ix == self.selected;
                card.focus = handles.get(ix).cloned();
                card.on_select = self.on_select.clone().map(|f| -> ClickHandler {
                    Rc::new(move |_, w, cx| f(ix, w, cx))
                });
                card
            })
            .collect();
        let root = div().id(self.id).flex().flex_col().gap(px(SPACE_1_5));
        focus::roving_keys(root, handles, self.selected, true, self.on_select).children(cards)
    }
}

/// The type-to-confirm field of a destructive dialog: "Type `name` to delete it" above an
/// input with the `danger` border and halo. Put the editable input in it as a child, or show
/// a static `value`; enable the destructive button when the text equals `expected`.
#[derive(IntoElement)]
pub struct ConfirmInput {
    id: ElementId,
    expected: SharedString,
    verb: SharedString,
    value: Option<SharedString>,
    content: Vec<AnyElement>,
}

impl ConfirmInput {
    /// A field asking for `expected` to be typed.
    pub fn new(id: impl Into<ElementId>, expected: impl Into<SharedString>) -> Self {
        ConfirmInput {
            id: id.into(),
            expected: expected.into(),
            verb: "delete it".into(),
            value: None,
            content: Vec::new(),
        }
    }

    /// What typing it does, after "to" ("delete it" by default).
    pub fn verb(mut self, verb: impl Into<SharedString>) -> Self {
        self.verb = verb.into();
        self
    }

    /// Static text standing in for what was typed.
    pub fn value(mut self, text: impl Into<SharedString>) -> Self {
        self.value = Some(text.into());
        self
    }
}

impl ParentElement for ConfirmInput {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

impl RenderOnce for ConfirmInput {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let label = row()
            .gap(px(SPACE_1))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child("Type")
            .child(div().font_family(FONT_MONO).text_color(TEXT).child(self.expected.clone()))
            .child(SharedString::from(format!("to {}", self.verb)));
        let mut frame = InputFrame::new(self.id).danger(true).placeholder(self.expected);
        if let Some(v) = self.value {
            frame = frame.value(v);
        }
        col().gap(px(SPACE_1_5)).child(label).child(frame.children(self.content))
    }
}
