//! The overlay host: one entity at the root of a window that draws every popover, menu,
//! dialog and toast, and the [`Overlays`] handle views use to open them.
//!
//! What the host guarantees (`GRAPHITE.md`, "Popups"):
//!
//! - One popover or menu at a time: opening another closes the first.
//! - A click outside a popover closes it and goes no further, so the press does not also
//!   activate what was under the pointer (the trigger included: pressing it again closes).
//! - Dialogs sit on a scrim over the whole window; a press on the scrim closes the dialog;
//!   Tab and Shift-Tab cycle inside the top layer.
//! - Escape ([`keys::Cancel`]) closes the top layer only. Toasts never take Escape.
//! - Focus moves into a layer when it opens and back to what had it when it closes.
//! - Every layer enters and leaves with the motion helpers, and stays drawn (not clickable)
//!   until its exit has played. Layers stack popover 20 < scrim 40 < dialog 50 < toast 60; a
//!   popover opened from a dialog stacks above it.
//!
//! A screen opens things from its handlers:
//!
//! ```ignore
//! Button::new("new", "New").primary().popover_anchor("agents-new")
//!     .on_click(|_, window, cx| {
//!         let anchor = Anchor::of("agents-new", window, cx);
//!         overlays(window, cx).open_menu(anchor, Placement::below().end(), |_, _| {
//!             Menu::new("new-menu").width(384.).item(MenuItem::new("agent", "Agent").on_click(..))
//!         }, window, cx);
//!     })
//! overlays(window, cx).open_dialog("deploy", |_, _| Dialog::new("deploy", "Deploy v15").into_any_element(), window, cx);
//! overlays(window, cx).toast(Toast::new("Release captain archived").undo(|w, cx| ..), cx);
//! ```

mod anchor;
mod render;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, ElementId, Entity, FocusHandle, Global,
    IntoElement as _, ParentElement as _, Render, Task, WeakEntity, Window, WindowId,
};

pub use anchor::{
    Align, Anchor, AnchorProbe, Placement, PopoverAnchorExt, Side, WINDOW_MARGIN, place,
};
pub(crate) use anchor::Placed;

use crate::menu::Menu;
use crate::motion::Presence;
use crate::popover::Popover;
use crate::toast::Toast;

/// One open (or leaving) layer, for closing it later: see [`current_layer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayerId(u64);

thread_local! {
    static CURRENT: Cell<Option<LayerId>> = const { Cell::new(None) };
}

/// The layer whose content is being built right now, if any. Components made inside an
/// overlay's build closure (a [`Dialog`](crate::dialog::Dialog), a [`Menu`]) call it to know
/// which layer to close; custom content can do the same.
pub fn current_layer() -> Option<LayerId> {
    CURRENT.with(Cell::get)
}

pub(crate) fn with_layer<R>(id: LayerId, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT.with(|c| c.replace(Some(id)));
    let out = f();
    CURRENT.with(|c| c.set(prev));
    out
}

/// What a layer's content is built from, once per frame.
pub(crate) type Build = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// A user callback with no arguments but the window.
pub(crate) type Callback = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub(crate) enum Kind {
    Popover { anchor: Anchor, placement: Placement },
    Dialog,
    /// Covers the whole window over [`SCRIM_STRONG`](crate::theme::SCRIM_STRONG); fades only.
    /// The content takes every press (the host's scrim is under it and not clickable).
    Full,
}

pub(crate) struct Layer {
    pub id: LayerId,
    pub key: ElementId,
    pub kind: Kind,
    pub build: Build,
    /// Tracked by the layer's panel: Escape and Tab look at it to know where focus is.
    pub focus: FocusHandle,
    pub restore: Option<FocusHandle>,
    pub presence: Presence,
    pub open: bool,
    /// Called once when the layer closes (any way but being replaced by a reopen).
    pub on_close: Option<Callback>,
}

impl Layer {
    fn is_popover(&self) -> bool {
        matches!(self.kind, Kind::Popover { .. })
    }
}

pub(crate) struct ToastSlot {
    pub id: LayerId,
    pub toast: Toast,
    pub presence: Presence,
    pub open: bool,
    timer: Option<Task<()>>,
    deadline: Option<Instant>,
    remaining: Duration,
}

#[derive(Default)]
struct Hosts(std::collections::HashMap<WindowId, WeakEntity<OverlayHost>>);

impl Global for Hosts {}

/// The root of a window that draws its overlays. Make it the window's root view around the
/// app's content ([`OverlayHost::wrap`]; [`crate::run_app`] does), then reach it from any
/// view with [`overlays`].
pub struct OverlayHost {
    /// The window's fallback focus: keys reach the host's handlers even when nothing in the
    /// content has focus.
    pub(crate) focus: FocusHandle,
    pub(crate) content: Option<AnyView>,
    pub(crate) layers: Vec<Layer>,
    pub(crate) toast: Option<ToastSlot>,
    next: u64,
}

impl OverlayHost {
    /// A host with no content yet, registered as the overlay host of `window`. Create it with
    /// `cx.new(|cx| OverlayHost::new(window, cx))` and give it content.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = window.window_handle().window_id();
        let me = cx.weak_entity();
        cx.default_global::<Hosts>().0.insert(id, me);
        OverlayHost { focus: cx.focus_handle(), content: None, layers: Vec::new(), toast: None, next: 0 }
    }

    /// Draw `view` under the overlays.
    pub fn with_content(mut self, view: impl Into<AnyView>) -> Self {
        self.content = Some(view.into());
        self
    }

    /// Replace the content under the overlays.
    pub fn set_content(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.content = Some(view.into());
        cx.notify();
    }

    /// A host around `content`, to return as a window's root view.
    pub fn wrap(content: impl Into<AnyView>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let content = content.into();
        cx.new(|cx| OverlayHost::new(window, cx).with_content(content))
    }

    fn next_id(&mut self) -> LayerId {
        self.next += 1;
        LayerId(self.next)
    }

    /// Index of the top layer that is open (not leaving).
    pub(crate) fn top_open(&self) -> Option<usize> {
        self.layers.iter().rposition(|l| l.open)
    }

    /// What focus should return to when a new layer closes: the focused element, unless it
    /// sits in a popover that is about to close, whose own target is used instead.
    fn restore_target(&self, window: &Window, cx: &App) -> Option<FocusHandle> {
        for l in self.layers.iter().rev().filter(|l| l.open && l.is_popover()) {
            if l.focus.contains_focused(window, cx) {
                return l.restore.clone();
            }
        }
        window.focused(cx)
    }

    fn open_layer(
        &mut self,
        key: ElementId,
        kind: Kind,
        build: Build,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> LayerId {
        self.open_layer_with(key, kind, build, None, window, cx)
    }

    fn open_layer_with(
        &mut self,
        key: ElementId,
        kind: Kind,
        build: Build,
        on_close: Option<Callback>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> LayerId {
        let restore = self.restore_target(window, cx);
        // One popover at a time; a dialog also closes open popovers.
        for l in self.layers.iter_mut().filter(|l| l.open && l.is_popover()) {
            l.open = false;
            l.presence.set(false);
        }
        // Reopening the same key replaces the old layer at once.
        self.layers.retain(|l| l.key != key || l.open);
        if let Some(ix) = self.layers.iter().position(|l| l.open && l.key == key) {
            self.layers.remove(ix);
        }
        let id = self.next_id();
        let focus = cx.focus_handle();
        let mut presence = Presence::default();
        presence.set(true);
        window.focus(&focus);
        self.layers.push(Layer { id, key, kind, build, focus, restore, presence, open: true, on_close });
        cx.notify();
        id
    }

    /// Start the exit of layer `ix` and give focus back if it was inside.
    pub(crate) fn close_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = self.layers.get_mut(ix) else { return };
        if !l.open {
            return;
        }
        l.open = false;
        l.presence.set(false);
        let inside = l.focus.contains_focused(window, cx) || window.focused(cx).is_none();
        let restore = l.restore.take();
        if let Some(f) = l.on_close.take() {
            // After this update: the callback may reach the host again (close, reopen).
            window.defer(cx, move |window, cx| f(window, cx));
        }
        if inside {
            window.focus(restore.as_ref().unwrap_or(&self.focus));
        }
        cx.notify();
    }

    pub(crate) fn close_id(&mut self, id: LayerId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.layers.iter().position(|l| l.id == id) {
            self.close_at(ix, window, cx);
        }
    }

    fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(ix) = self.top_open() {
            self.close_at(ix, window, cx);
        }
    }

    /// Escape, before anything under the focus sees it: when focus is outside the top layer
    /// (a toast button, the page), close that layer here. Inside, the layer's own content and
    /// panel handle it on the way up.
    pub(crate) fn capture_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.top_open() else { return false };
        if self.layers[ix].focus.contains_focused(window, cx) {
            return false;
        }
        self.close_at(ix, window, cx);
        true
    }

    /// Tab and Shift-Tab: through the whole window, or round the top layer while one is open.
    pub(crate) fn move_focus(&mut self, next: bool, window: &mut Window, cx: &mut Context<Self>) {
        crate::focus::set_focus_visible(true, window, cx);
        let Some(ix) = self.top_open() else {
            if next { window.focus_next() } else { window.focus_prev() }
            return;
        };
        let panel = self.layers[ix].focus.clone();
        let inside = |window: &Window, cx: &App| {
            window.focused(cx).is_some_and(|f| f != panel && panel.contains(&f, window))
        };
        if !panel.contains_focused(window, cx) {
            window.focus(&panel);
        }
        if next { window.focus_next() } else { window.focus_prev() }
        if inside(window, cx) {
            return;
        }
        // Left the layer: wrap to its first (Tab) or last (Shift-Tab) stop.
        window.focus(&panel);
        window.focus_next();
        if !inside(window, cx) {
            window.focus(&panel);
            return;
        }
        if !next {
            let mut last = window.focused(cx);
            for _ in 0..256 {
                window.focus_next();
                if !inside(window, cx) {
                    break;
                }
                let now = window.focused(cx);
                if now == last {
                    break;
                }
                last = now;
            }
            if let Some(h) = last {
                window.focus(&h);
            }
        }
    }

    pub(crate) fn show_toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        let id = self.next_id();
        let mut presence = Presence::default();
        presence.set(true);
        let remaining = toast.stay;
        self.toast = Some(ToastSlot {
            id,
            toast,
            presence,
            open: true,
            timer: None,
            deadline: None,
            remaining,
        });
        self.arm_toast(cx);
        cx.notify();
    }

    fn arm_toast(&mut self, cx: &mut Context<Self>) {
        let Some(slot) = self.toast.as_mut() else { return };
        let (id, wait) = (slot.id, slot.remaining);
        slot.deadline = Some(cx.background_executor().now() + wait);
        slot.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |host, cx| {
                if host.toast.as_ref().is_some_and(|t| t.id == id) {
                    host.dismiss_toast(cx);
                }
            })
            .ok();
        }));
    }

    /// Pause the countdown while the pointer is over the toast; resume with what was left.
    pub(crate) fn hover_toast(&mut self, hovered: bool, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let Some(slot) = self.toast.as_mut().filter(|t| t.open) else { return };
        if hovered {
            if let Some(deadline) = slot.deadline.take() {
                slot.remaining = deadline.saturating_duration_since(now);
            }
            slot.timer = None;
        } else if slot.timer.is_none() {
            self.arm_toast(cx);
        }
    }

    pub(crate) fn dismiss_toast(&mut self, cx: &mut Context<Self>) {
        if let Some(slot) = self.toast.as_mut().filter(|t| t.open) {
            slot.open = false;
            slot.presence.set(false);
            slot.timer = None;
            cx.notify();
        }
    }

    /// Dismiss the toast and hand back its Undo, if it has one and is still up.
    pub(crate) fn take_undo(&mut self, cx: &mut Context<Self>) -> Option<Callback> {
        let undo = self.toast.as_ref().filter(|t| t.open)?.toast.undo.clone()?;
        self.dismiss_toast(cx);
        Some(undo)
    }
}

/// A handle on the overlay host of a window, from [`overlays`]. Cheap to clone. When the
/// window has no host every call does nothing (and `is_open` is false).
///
/// Do not call it from inside a build closure: the host is drawing then.
#[derive(Clone)]
pub struct Overlays {
    host: Option<WeakEntity<OverlayHost>>,
}

/// The overlay host of `window`.
pub fn overlays(window: &Window, cx: &App) -> Overlays {
    let id = window.window_handle().window_id();
    Overlays { host: cx.try_global::<Hosts>().and_then(|h| h.0.get(&id).cloned()) }
}

impl Overlays {
    fn update<R>(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut OverlayHost, &mut Context<OverlayHost>) -> R,
    ) -> Option<R> {
        let host = self.host.as_ref()?.upgrade()?;
        Some(host.update(cx, f))
    }

    fn read<R>(&self, cx: &App, f: impl FnOnce(&OverlayHost) -> R) -> Option<R> {
        let host = self.host.as_ref()?.upgrade()?;
        Some(f(host.read(cx)))
    }

    /// The window has an overlay host.
    pub fn is_mounted(&self, cx: &App) -> bool {
        self.read(cx, |_| ()).is_some()
    }

    /// Open a popover built by `build` (usually a [`Popover`]) at `anchor`. Closes any other
    /// popover. Its id, for [`Overlays::is_open`], is the anchor's key.
    pub fn open_popover(
        &self,
        anchor: Anchor,
        placement: Placement,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LayerId> {
        self.update(cx, |host, cx| {
            let key = anchor
                .key
                .clone()
                .unwrap_or_else(|| ElementId::NamedInteger("popover".into(), host.next + 1));
            host.open_layer(key, Kind::Popover { anchor, placement }, Rc::new(build), window, cx)
        })
    }

    /// Open a [`Menu`] in a popover at `anchor`. Choosing an item closes it.
    pub fn open_menu(
        &self,
        anchor: Anchor,
        placement: Placement,
        build: impl Fn(&mut Window, &mut App) -> Menu + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LayerId> {
        self.open_popover(
            anchor,
            placement,
            move |window, cx| {
                let menu = build(window, cx);
                Popover::new().width_opt(menu.preferred_width()).child(menu).into_any_element()
            },
            window,
            cx,
        )
    }

    /// Open a dialog over the scrim. `build` usually returns a
    /// [`Dialog`](crate::dialog::Dialog); other content must `occlude()` itself.
    pub fn open_dialog(
        &self,
        id: impl Into<ElementId>,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LayerId> {
        let key = id.into();
        self.update(cx, |host, cx| host.open_layer(key, Kind::Dialog, Rc::new(build), window, cx))
    }

    /// Open a view the caller owns as a dialog layer over the scrim. The view's render
    /// usually returns a [`Dialog`](crate::dialog::Dialog), which then knows its layer: its
    /// close button, Escape and Cancel close it. `on_close` runs once the layer closes, however
    /// that happens (Escape, the close button, a press on the scrim, [`Overlays::close`]), so
    /// the owner can drop the view or tell its router; it does not run when the same `id` is
    /// opened again and replaces the layer.
    pub fn open_dialog_view(
        &self,
        id: impl Into<ElementId>,
        view: impl Into<AnyView>,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LayerId> {
        let key = id.into();
        let view: AnyView = view.into();
        let build: Build = Rc::new(move |_, _| view.clone().into_any_element());
        let on_close: Callback = Rc::new(on_close);
        self.update(cx, |host, cx| host.open_layer_with(key, Kind::Dialog, build, Some(on_close), window, cx))
    }

    /// Open a view the caller owns over the whole window, on a dark scrim (an image viewer, a
    /// full-screen preview). It fades in and out with the dialog timings and does not move.
    /// The view is laid out at the window's size and takes every press itself: to close on a
    /// press outside its content it calls [`Overlays::close`] with `id`. Escape, focus moving
    /// in and back, Tab staying inside, and `on_close` work as for
    /// [`Overlays::open_dialog_view`]. The view should focus a handle of its own when it is
    /// first drawn, so its key bindings apply.
    pub fn open_full_view(
        &self,
        id: impl Into<ElementId>,
        view: impl Into<AnyView>,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LayerId> {
        let key = id.into();
        let view: AnyView = view.into();
        let build: Build = Rc::new(move |_, _| view.clone().into_any_element());
        let on_close: Callback = Rc::new(on_close);
        self.update(cx, |host, cx| host.open_layer_with(key, Kind::Full, build, Some(on_close), window, cx))
    }

    /// Show `toast` at the bottom centre, replacing the one that is up.
    pub fn toast(&self, toast: Toast, cx: &mut App) {
        self.update(cx, |host, cx| host.show_toast(toast, cx));
    }

    /// Take the toast down now.
    pub fn dismiss_toast(&self, cx: &mut App) {
        self.update(cx, |host, cx| host.dismiss_toast(cx));
    }

    /// A toast is up.
    pub fn has_toast(&self, cx: &App) -> bool {
        self.read(cx, |h| h.toast.as_ref().is_some_and(|t| t.open)).unwrap_or(false)
    }

    /// Close the popover or dialog opened as `id`.
    pub fn close(&self, id: impl Into<ElementId>, window: &mut Window, cx: &mut App) {
        let key = id.into();
        self.update(cx, |host, cx| {
            if let Some(ix) = host.layers.iter().position(|l| l.open && l.key == key) {
                host.close_at(ix, window, cx);
            }
        });
    }

    /// Close one layer (from [`current_layer`]).
    pub fn close_layer(&self, layer: LayerId, window: &mut Window, cx: &mut App) {
        self.update(cx, |host, cx| host.close_id(layer, window, cx));
    }

    /// Close the top popover or dialog.
    pub fn close_top(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |host, cx| {
            if let Some(ix) = host.top_open() {
                host.close_at(ix, window, cx);
            }
        });
    }

    /// Close every popover and dialog, top first.
    pub fn close_all(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |host, cx| host.close_all(window, cx));
    }

    /// The popover or dialog opened as `id` is open (not closed or leaving).
    pub fn is_open(&self, id: impl Into<ElementId>, cx: &App) -> bool {
        let key = id.into();
        self.read(cx, |h| h.layers.iter().any(|l| l.open && l.key == key)).unwrap_or(false)
    }

    /// Number of open popovers and dialogs.
    pub fn open_count(&self, cx: &App) -> usize {
        self.read(cx, |h| h.layers.iter().filter(|l| l.open).count()).unwrap_or(0)
    }

    /// Whether layer `layer` is a popover (menus close it when an item is chosen).
    pub(crate) fn is_popover_layer(&self, layer: LayerId, cx: &App) -> bool {
        self.read(cx, |h| h.layers.iter().any(|l| l.id == layer && l.is_popover())).unwrap_or(false)
    }
}

impl Render for OverlayHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.render_host(window, cx)
    }
}
