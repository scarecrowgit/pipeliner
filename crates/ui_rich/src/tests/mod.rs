//! The editors in a test window: typed input through the test input handler, key bindings,
//! events. GPUI's test platform has no real text shaping, so geometry (wrapping, vertical
//! movement) is tested on the buffer with a fake layout instead.

mod composer;
mod composer_spans;
mod geometry;
mod tab_stops;
mod text_area;
mod text_input;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyView, Context, Entity, EventEmitter, FocusHandle, IntoElement, Render, Subscription,
    TestAppContext, VisualTestContext, Window, div, prelude::*,
};

/// Events a host saw, in order.
pub(crate) type Log<E> = Rc<RefCell<Vec<E>>>;

/// A window root holding one editor (any view) and a second focus target for blur tests.
pub(crate) struct Host {
    pub view: AnyView,
    pub other: FocusHandle,
    _sub: Subscription,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .flex()
            .flex_col()
            .child(div().w(gpui::px(400.)).child(self.view.clone()))
            .child(div().id("other").track_focus(&self.other).size_4())
    }
}

/// Open a window with the editor `make` builds; returns it, its event log and the context.
/// The editor is focused.
pub(crate) fn open<V, E>(
    cx: &mut TestAppContext,
    make: impl FnOnce(&mut Context<V>) -> V + 'static,
) -> (Entity<V>, Log<E>, FocusHandle, &mut VisualTestContext)
where
    V: Render + EventEmitter<E> + gpui::Focusable,
    E: Clone + 'static,
{
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        crate::bind_keys(cx);
    });
    let log: Log<E> = Rc::default();
    let l = log.clone();
    let slot: Rc<RefCell<Option<Entity<V>>>> = Rc::default();
    let s = slot.clone();
    let (host, cx) = cx.add_window_view(move |_, cx| {
        let view = cx.new(make);
        let sub = cx.subscribe(&view, move |_, _, e: &E, _| l.borrow_mut().push(e.clone()));
        *s.borrow_mut() = Some(view.clone());
        Host {
            view: view.into(),
            other: cx.focus_handle(),
            _sub: sub,
        }
    });
    let Some(view) = slot.borrow_mut().take() else {
        panic!("view not built")
    };
    let other = host.read_with(cx, |h, _| h.other.clone());
    let handle = view.read_with(cx, |v, cx| v.focus_handle(cx));
    cx.update(|window, _| {
        window.activate_window();
        handle.focus(window);
    });
    cx.run_until_parked();
    (view, log, other, cx)
}

/// Drop everything logged so far.
pub(crate) fn clear<E>(log: &Log<E>) {
    log.borrow_mut().clear();
}
