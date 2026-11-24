//! Home in a test window over the fixture backend, under a stand-in for the shell's header,
//! driven by simulated clicks and keys.

mod lists;
mod page;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, Entity, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px,
    size,
};
use hub::fixtures::{FixtureApi, World};
use hub::{Hub, Route};
use ui::OverlayHost;
use ui::prelude::*;

use crate::HomeView;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;

/// The shell's part: the header with the view's badges and actions, the view under it.
struct Stage {
    view: Entity<HomeView>,
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, badges, actions) =
            self.view.update(cx, |v, cx| (v.title(cx), v.header_badges(window, cx), v.header_actions(window, cx)));
        ui::root()
            .flex()
            .flex_col()
            .child(
                row()
                    .flex_none()
                    .h(px(HEADER_HEIGHT))
                    .gap(px(SPACE_2))
                    .child(title)
                    .children(badges)
                    .child(grow())
                    .children(actions),
            )
            .child(div().flex_1().min_h_0().flex().child(self.view.clone()))
    }
}

/// An open Home.
pub(crate) struct Open {
    #[allow(dead_code)]
    pub api: Arc<FixtureApi>,
    pub view: Entity<HomeView>,
    _hub: Entity<Hub>,
}

/// Home over the Acme world.
pub(crate) fn open(cx: &mut TestAppContext) -> (Open, &mut VisualTestContext) {
    open_with(cx, |_| {})
}

/// Like [`open`], after `edit` changed the world.
pub(crate) fn open_with(
    cx: &mut TestAppContext,
    edit: impl FnOnce(&mut World),
) -> (Open, &mut VisualTestContext) {
    open_told(cx, edit, &[])
}

/// Like [`open_with`], where the fixture fails the first call of each query in `fail` (the
/// names of `FixtureApi::fail_next`).
pub(crate) fn open_told<'a>(
    cx: &'a mut TestAppContext,
    edit: impl FnOnce(&mut World),
    fail: &[&str],
) -> (Open, &'a mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    // After `ui::init`, which reads the preference again: popups appear and leave at once.
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    api.with_world(edit);
    for query in fail {
        api.fail_next(query);
    }
    let hub = cx.update(|cx| Hub::new_fixture(api.clone(), cx));
    cx.update(|cx| hub::navigate(Route::Home, cx));
    let (view, cx) = mount(cx);
    (Open { api, view, _hub: hub }, cx)
}

/// A window with a new Home in it.
pub(crate) fn mount(cx: &mut TestAppContext) -> (Entity<HomeView>, &mut VisualTestContext) {
    let slot = Rc::new(RefCell::new(None));
    let put = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| {
            let mut view = HomeView::new(window, cx);
            view.now = Some(NOW);
            view
        });
        *put.borrow_mut() = Some(view.clone());
        let stage = cx.new(|_| Stage { view });
        OverlayHost::new(window, cx).with_content(stage)
    });
    let Some(view) = slot.borrow_mut().take() else { panic!("the view was not built") };
    cx.simulate_resize(size(px(1440.), px(1600.)));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (view, cx)
}

/// Clicks the element drawn with debug selector `selector`.
pub(crate) fn click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let bounds = cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// An element with debug selector `selector` was drawn in the last frame.
pub(crate) fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).is_some()
}

/// Presses keys (`"j"`, `"enter"`), space-separated.
pub(crate) fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
}

/// The route the app is on.
pub(crate) fn route(cx: &mut VisualTestContext) -> Route {
    cx.update(|_, cx| hub::router(cx).read(cx).current().clone())
}

/// `debug_bounds` wants a `'static` selector; tests leak their few.
fn leak(selector: &str) -> &'static str {
    Box::leak(selector.to_string().into_boxed_str())
}
