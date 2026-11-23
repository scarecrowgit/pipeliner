//! Toasts: a one-line result at the bottom centre, above everything, with an optional Undo.
//! Shown through [`Overlays::toast`](crate::overlay::Overlays::toast); one at a time.

use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyElement, App, Hsla, IntoElement, SharedString, WeakEntity, Window, div, prelude::*, px};

use crate::assets::{IconName, icon};
use crate::components::{Button, IconButton};
use crate::motion::PopFrame;
use crate::overlay::{Callback, OverlayHost};
use crate::theme::*;

/// What a toast says: the result in strong text ("2 chats archived", not "Archiving…"), an
/// optional muted detail, an optional status icon, and after anything destructive one action,
/// Undo (also on Cmd/Ctrl-Z). It stays [`TOAST_STAY`], paused while hovered.
#[derive(Clone)]
pub struct Toast {
    pub(crate) title: SharedString,
    pub(crate) detail: Option<SharedString>,
    pub(crate) icon: Option<(IconName, Hsla)>,
    pub(crate) undo: Option<Callback>,
    pub(crate) undo_kbd: SharedString,
    pub(crate) stay: Duration,
}

impl Toast {
    /// A toast reading `title`, with a success check.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Toast {
            title: title.into(),
            detail: None,
            icon: Some((IconName::Check, SUCCESS)),
            undo: None,
            undo_kbd: "⌘Z".into(),
            stay: TOAST_STAY,
        }
    }

    /// Muted text after the title, joined by " · ".
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Another leading icon and colour.
    pub fn icon(mut self, name: IconName, color: Hsla) -> Self {
        self.icon = Some((name, color));
        self
    }

    /// A failure: the alert icon in `danger`.
    pub fn error(self) -> Self {
        self.icon(IconName::Alert, DANGER)
    }

    /// No leading icon.
    pub fn no_icon(mut self) -> Self {
        self.icon = None;
        self
    }

    /// Offer Undo: the button and Cmd/Ctrl-Z call `f` and take the toast down.
    pub fn undo(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.undo = Some(Rc::new(f));
        self
    }

    /// The key cap on the Undo button ("⌘Z" by default).
    pub fn undo_kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.undo_kbd = keys.into();
        self
    }

    /// Stay up for `d` instead of four seconds.
    pub fn stay(mut self, d: Duration) -> Self {
        self.stay = d;
        self
    }
}

/// The toast of `slot` at frame `f`, bottom centre.
pub(crate) fn render_toast(
    slot: &crate::overlay::ToastSlot,
    f: PopFrame,
    host: &WeakEntity<OverlayHost>,
) -> AnyElement {
    let t = &slot.toast;
    let dismiss = {
        let host = host.clone();
        move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            host.update(cx, |h, cx| h.dismiss_toast(cx)).ok();
        }
    };
    let undo = t.undo.is_some().then(|| {
        let host = host.clone();
        Button::new("toast-undo", "Undo").small().kbd(t.undo_kbd.clone()).on_click(
            move |_, window, cx| {
                if let Some(undo) = host.update(cx, |h, cx| h.take_undo(cx)).ok().flatten() {
                    undo(window, cx);
                }
            },
        )
    });
    let hover = {
        let host = host.clone();
        move |hovered: &bool, _: &mut Window, cx: &mut App| {
            host.update(cx, |h, cx| h.hover_toast(*hovered, cx)).ok();
        }
    };
    let text = div()
        .flex()
        .flex_none()
        .gap(px(SPACE_1))
        .child(div().type_style(BODY_STRONG).child(t.title.clone()))
        .when_some(t.detail.clone(), |d, detail| {
            d.child(div().text_color(TEXT_MUTED).child(SharedString::from(format!("· {detail}"))))
        });
    let toast = div()
        .id("toast")
        .debug_selector(|| "toast".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(SPACE_3))
        .py(px(SPACE_1_5))
        .pl(px(SPACE_3_5))
        .pr(px(SPACE_1_5))
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(RADIUS_LG))
        .bg(SURFACE_RAISED)
        .text_color(TEXT)
        .type_style(BODY)
        .whitespace_nowrap()
        .opacity(f.opacity)
        .occlude()
        .when(!f.leaving, |d| d.on_hover(hover))
        .when_some(t.icon, |d, (name, color)| d.child(icon(name, 14., color)))
        .child(text)
        .children(undo)
        .child(IconButton::new("toast-dismiss", IconName::X).small().tooltip("Dismiss").on_click(dismiss));
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(SPACE_6 - f.dy))
        .flex()
        .justify_center()
        .child(toast)
        .into_any_element()
}
