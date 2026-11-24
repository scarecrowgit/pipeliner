//! Drawing the host: the content, then each layer in stack order on a deferred layer of its
//! own, then the toast.

use gpui::{
    AnyElement, App, Bounds, Context, Div, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, KeyDownEvent, LayoutId, Pixels, RenderOnce, Stateful, WeakEntity, Window, deferred,
    div, prelude::*, px,
};

use super::{Build, Kind, Layer, LayerId, OverlayHost, Placed, with_layer};
use crate::focus::set_focus_visible;
use crate::keys::{self, Cancel, FocusNext, FocusPrev, Undo};
use crate::motion::{Pop, PopFrame};
use crate::theme::{SCRIM, SCRIM_STRONG, Z_DIALOG, Z_POPOVER, Z_TOAST};

/// A layer's content, built when it is laid out (not while the host renders, so a build
/// closure may read the host) with [`super::current_layer`] set.
#[derive(IntoElement)]
struct LayerContent {
    id: LayerId,
    build: Build,
}

impl RenderOnce for LayerContent {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let build = self.build;
        let child = with_layer(self.id, || build(window, cx));
        LayerScope { id: self.id, child }
    }
}

/// Keeps [`super::current_layer`] set while its child is laid out and painted, so a view
/// placed in a layer (its render runs during layout, after the build closure returned)
/// knows the layer too.
struct LayerScope {
    id: LayerId,
    child: AnyElement,
}

impl IntoElement for LayerScope {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for LayerScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = &mut self.child;
        (with_layer(self.id, || child.request_layout(window, cx)), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        with_layer(self.id, || child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        with_layer(self.id, || child.paint(window, cx));
    }
}

fn closer(
    host: &WeakEntity<OverlayHost>,
    id: LayerId,
) -> impl Fn(&mut Window, &mut App) + Clone + 'static {
    let host = host.clone();
    move |window: &mut Window, cx: &mut App| {
        host.update(cx, |h, cx| h.close_id(id, window, cx)).ok();
    }
}

/// A key that says the user is working the keyboard (not a bare modifier).
fn is_navigation_key(ev: &KeyDownEvent) -> bool {
    !matches!(
        ev.keystroke.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function" | "capslock" | ""
    )
}

impl OverlayHost {
    pub(crate) fn render_host(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        // Drop what has finished leaving.
        self.layers.retain(|l| l.open || l.presence.frame(Pop::Dialog).is_some());
        if self.toast.as_ref().is_some_and(|t| !t.open && t.presence.frame(Pop::Toast).is_none()) {
            self.toast = None;
        }

        let host = cx.weak_entity();
        let mut animating = false;
        let mut z = 0usize;
        let mut layers: Vec<AnyElement> = Vec::new();
        for layer in &self.layers {
            let base = match layer.kind {
                Kind::Popover { .. } => Z_POPOVER,
                Kind::Dialog | Kind::Full => Z_DIALOG,
            } as usize;
            z = base.max(z + 1);
            if let Some((el, settled)) = render_layer(layer, &host) {
                animating |= !settled;
                layers.push(deferred(el).with_priority(z).into_any_element());
            }
        }
        let toast = self.toast.as_ref().and_then(|slot| {
            let frame = slot.presence.frame(Pop::Toast)?;
            animating |= !frame.settled();
            let el = crate::toast::render_toast(slot, frame, &host);
            Some(deferred(el).with_priority((Z_TOAST as usize).max(z + 1)))
        });
        if animating {
            window.request_animation_frame();
        }
        if window.focused(cx).is_none() {
            window.focus(&self.focus);
        }

        crate::root()
            .id("overlay-host")
            .track_focus(&self.focus)
            .relative()
            .capture_any_mouse_down(|_, window, cx| set_focus_visible(false, window, cx))
            .capture_key_down(|ev, window, cx| {
                if is_navigation_key(ev) {
                    set_focus_visible(true, window, cx);
                }
            })
            .capture_action(cx.listener(|this, _: &Cancel, window, cx| {
                if this.capture_cancel(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &FocusNext, window, cx| this.move_focus(true, window, cx)))
            .on_action(cx.listener(|this, _: &FocusPrev, window, cx| this.move_focus(false, window, cx)))
            .on_action(cx.listener(|this, _: &Undo, window, cx| match this.take_undo(cx) {
                Some(undo) => window.defer(cx, move |window, cx| undo(window, cx)),
                None => cx.propagate(),
            }))
            .children(self.content.clone())
            .children(layers)
            .children(toast)
    }
}

/// One layer and whether its animation has settled; `None` once it has left.
fn render_layer(layer: &Layer, host: &WeakEntity<OverlayHost>) -> Option<(AnyElement, bool)> {
    let id = layer.id;
    let close = closer(host, id);
    let content = LayerContent { id, build: layer.build.clone() };
    let panel = |f: PopFrame| {
        let close = close.clone();
        div()
            .id(("overlay-panel", id.0 as usize))
            .relative()
            .flex()
            .flex_col()
            .track_focus(&layer.focus)
            .key_context(keys::OVERLAY)
            .on_action(move |_: &Cancel, window, cx| close(window, cx))
            .opacity(f.opacity)
            .child(content)
            // While leaving it is still drawn but takes no clicks.
            .when(f.leaving, |d| d.child(div().absolute().inset_0().occlude()))
    };
    match &layer.kind {
        Kind::Popover { anchor, placement } => {
            let f = layer.presence.frame(Pop::Below)?;
            let placed = Placed {
                anchor: anchor.clone(),
                placement: *placement,
                toward: f.dy.abs(),
                child: panel(f).occlude().into_any_element(),
            };
            let backdrop = (!f.leaving).then(|| {
                let close = close.clone();
                div()
                    .id(("overlay-backdrop", id.0 as usize))
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_any_mouse_down(move |_, window, cx| {
                        close(window, cx);
                        cx.stop_propagation();
                    })
            });
            let el = div().absolute().inset_0().children(backdrop).child(placed);
            Some((el.into_any_element(), f.settled()))
        }
        Kind::Dialog => {
            let f = layer.presence.frame(Pop::Dialog)?;
            let s = layer.presence.frame(Pop::Scrim).unwrap_or(f);
            let scrim = div()
                .id(("overlay-scrim", id.0 as usize))
                .absolute()
                .inset_0()
                .bg(SCRIM)
                .opacity(s.opacity)
                .occlude()
                .when(!f.leaving, |d| {
                    let close = close.clone();
                    d.on_any_mouse_down(move |_, window, cx| {
                        close(window, cx);
                        cx.stop_propagation();
                    })
                });
            let stage = div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .child(panel(f).mt(px(f.dy)));
            let el = div().absolute().inset_0().child(scrim).child(stage);
            Some((el.into_any_element(), f.settled() && s.settled()))
        }
        Kind::Full => {
            // Fades only: a panel the size of the window has nowhere to travel.
            let f = layer.presence.frame(Pop::Scrim)?;
            let scrim = div()
                .id(("overlay-full-scrim", id.0 as usize))
                .absolute()
                .inset_0()
                .bg(SCRIM_STRONG)
                .opacity(f.opacity)
                .occlude();
            let el = div().absolute().inset_0().child(scrim).child(panel(f).absolute().inset_0().occlude());
            Some((el.into_any_element(), f.settled()))
        }
    }
}
