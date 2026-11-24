//! Drawing a composer: the card, the chips, the text with its placeholder, the reason line
//! and the controls row with the screen's slots.

use gpui::{
    AnyElement, App, ElementId, Entity, ExternalPaths, IntoElement, MouseButton, RenderOnce,
    SharedString, Window, div, prelude::*, px,
};
use ui::components::{Button, IconButton, Kbd};
use ui::dev::Probe;
use ui::prelude::*;

use super::chips::chips_row;
use super::{Composer, ComposerEvent, ComposerKind};
use crate::text::{Backspace, Delete, DeleteWordLeft, DeleteWordRight};

/// `"<name>-<part>"`: the id of a part of the composer called `name`.
pub(super) fn part(name: &str, part: &str) -> ElementId {
    ElementId::Name(format!("{name}-{part}").into())
}

/// A [`Composer`] drawn with the screen's controls in its slots. Build it in the screen's
/// render each frame: `composer_view(&composer).leading(agent_picker).trailing(tag)`.
/// Rendering the composer entity directly draws it with empty slots.
#[derive(IntoElement)]
pub struct ComposerView {
    composer: Entity<Composer>,
    leading: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
}

/// The view of `composer`, slots empty.
pub fn composer_view(composer: &Entity<Composer>) -> ComposerView {
    ComposerView { composer: composer.clone(), leading: Vec::new(), trailing: Vec::new() }
}

impl ComposerView {
    /// A control at the start of the row under the text (agent picker, model picker).
    pub fn leading(mut self, el: impl IntoElement) -> Self {
        self.leading.push(el.into_any_element());
        self
    }

    /// A control at the end of the row, before the attach and send buttons (a skill tag).
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing.push(el.into_any_element());
        self
    }
}

/// What one frame draws, read from the composer before anything borrows the app mutably.
struct Frame {
    name: SharedString,
    kind: ComposerKind,
    focused: bool,
    empty: bool,
    text: SharedString,
    disabled: Option<SharedString>,
    placeholder: SharedString,
    placeholder_hint: Option<SharedString>,
    hint: Option<SharedString>,
    send_label: SharedString,
    attach: bool,
    full: bool,
    busy: bool,
    can_send: bool,
}

impl Frame {
    fn of(c: &Composer, window: &Window, cx: &App) -> Self {
        let area = c.area.read(cx);
        Frame {
            name: c.name.clone(),
            kind: c.kind,
            focused: area.is_focused(window),
            empty: area.is_empty(),
            text: c.text.clone().into(),
            disabled: c.disabled.clone(),
            placeholder: c.placeholder.clone(),
            placeholder_hint: c.placeholder_hint.clone(),
            hint: c.hint.clone(),
            send_label: c.send_label.clone(),
            attach: c.attach_button,
            full: c.attachments.len() >= c.limits.max_count,
            busy: c.busy,
            can_send: c.can_send(),
        }
    }

    /// Horizontal padding of the text and the reason line.
    fn pad_x(&self) -> f32 {
        match self.kind {
            ComposerKind::Chat => SPACE_3_5,
            ComposerKind::Issue => SPACE_3,
        }
    }
}

impl RenderOnce for ComposerView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let entity = self.composer;
        if entity.read(cx).overlay_out_of_step(window, cx) {
            let e = entity.clone();
            window.defer(cx, move |window, cx| e.update(cx, |c, cx| c.sync_overlay(window, cx)));
        }
        let f = Frame::of(entity.read(cx), window, cx);
        let area = entity.read(cx).area.clone();
        let chips = chips_row(&entity, window, cx);
        let border = motion::fade_color(
            part(&f.name, "border"),
            if f.focused { ACCENT_RING } else { LINE_STRONG },
            window,
            cx,
        );

        let text = text_block(&f, area.clone());
        let reason = f.disabled.clone().map(|r| {
            row()
                .gap(px(SPACE_1_5))
                .px(px(f.pad_x()))
                .pb(px(SPACE_2))
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(icon(IconName::Lock, 12., TEXT_MUTED))
                .child(r)
        });
        let controls = controls_row(&f, &entity, self.leading, self.trailing);

        let on = |left: bool| {
            let e = entity.clone();
            move |cx: &mut App| e.update(cx, |c, cx| c.select_mention_for_delete(left, cx))
        };
        let (bs, dwl, del, dwr) = (on(true), on(true), on(false), on(false));
        let back = entity.clone();
        let drop_to = entity.clone();
        let focus_area = area.clone();
        col()
            .id(part(&f.name, "card"))
            .w_full()
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(border)
            .rounded(px(RADIUS_XL))
            .when(f.focused, |d| d.shadow(input_glow()))
            .when(f.disabled.is_none(), |d| {
                d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    focus_area.read(cx).focus(window)
                })
            })
            .capture_action(move |_: &Backspace, _, cx| bs(cx))
            .capture_action(move |_: &DeleteWordLeft, _, cx| dwl(cx))
            .capture_action(move |_: &Delete, _, cx| del(cx))
            .capture_action(move |_: &DeleteWordRight, _, cx| dwr(cx))
            .on_action(move |_: &Backspace, _, cx| back.update(cx, |c, cx| c.backspace_at_start(cx)))
            .on_drop(move |paths: &ExternalPaths, _, cx| {
                let paths = paths.paths().to_vec();
                drop_to.update(cx, |c, cx| c.emit(ComposerEvent::FilesDropped(paths), cx));
            })
            .children(chips)
            .child(text)
            .children(reason)
            .child(controls)
    }
}

/// The text area with the placeholder drawn over it while it is empty.
fn text_block(f: &Frame, area: Entity<crate::TextArea>) -> impl IntoElement {
    let pad = f.pad_x();
    let min_h = match f.kind {
        ComposerKind::Chat => 44.,
        ComposerKind::Issue => 56.,
    };
    let placeholder = f.empty.then(|| {
        row()
            .absolute()
            .top(px(SPACE_2_5))
            .left(px(pad))
            .right(px(pad))
            .gap(px(SPACE_1_5))
            .overflow_hidden()
            .child(div().flex_none().text_color(TEXT_MUTED).child(f.placeholder.clone()))
            .when_some(f.placeholder_hint.clone(), |d, h| {
                d.child(truncate(h).text_color(TEXT_DISABLED))
            })
    });
    div()
        .relative()
        .flex()
        .flex_col()
        .px(px(pad))
        .py(px(SPACE_2_5))
        .min_h(px(min_h))
        .child(area)
        .children(placeholder)
        .probe_value(format!("{}-text", f.name), f.text.clone())
}

/// The row under the text: slots, hint, attach and send (or stop).
fn controls_row(
    f: &Frame,
    entity: &Entity<Composer>,
    leading: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
) -> impl IntoElement {
    let name = &f.name;
    let off = f.disabled.is_some();
    let attach = f.attach.then(|| {
        let e = entity.clone();
        IconButton::new(part(name, "attach"), IconName::Image)
            .small()
            .tooltip("Attach images")
            .disabled(off || f.full)
            .on_click(move |_, _, cx| e.update(cx, |c, cx| c.emit(ComposerEvent::AttachRequested, cx)))
            .probe(format!("{name}-attach"))
    });
    let send = if f.busy {
        let e = entity.clone();
        Button::new(part(name, "stop"), "Stop")
            .small()
            .secondary()
            .on_click(move |_, _, cx| e.update(cx, |c, cx| c.emit(ComposerEvent::Stop, cx)))
            .probe(format!("{name}-stop"))
            .into_any_element()
    } else {
        let e = entity.clone();
        Button::new(part(name, "send"), f.send_label.clone())
            .small()
            .secondary()
            .when(f.kind == ComposerKind::Chat, |b| b.icon(IconName::ArrowUp))
            .disabled(!f.can_send)
            .on_click(move |_, window, cx| {
                e.update(cx, |c, cx| {
                    c.submit(cx);
                    c.focus(window, cx);
                })
            })
            .probe(format!("{name}-send"))
            .into_any_element()
    };
    let hint = f.hint.clone().map(|h| {
        div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(h)
    });
    let base = row().px(px(SPACE_2)).pb(px(SPACE_2)).gap(px(SPACE_1_5));
    match f.kind {
        ComposerKind::Chat => base
            .children(leading)
            .child(grow())
            .children(trailing)
            .children(hint)
            .children(attach)
            .child(send),
        ComposerKind::Issue => {
            let note = (f.attach && f.hint.is_none() && !off).then(|| {
                div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child("Paste or drop images")
            });
            base.children(attach)
                .children(note)
                .children(leading)
                .child(grow())
                .children(trailing)
                .child(
                    row()
                        .flex_none()
                        .gap(px(SPACE_1))
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .children(hint)
                        .child(Kbd::new("⌘↵")),
                )
                .child(send)
        }
    }
}
