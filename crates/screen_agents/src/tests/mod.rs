//! The agents view in a test window over the fixture backend, under a stand-in for the shell's
//! header and inside an overlay host, driven by simulated clicks and keys.

mod actions;
mod list;
mod open;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px, size};
use hub::fixtures::{FixtureApi, World};
use hub::model::RunnableRef;
use hub::{Hub, Route};
use ui::OverlayHost;
use ui::prelude::*;

use super::AgentsView;

/// The fixed clock of the test world.
const NOW: i64 = 1_795_000_000_000;

/// The shell's part: the header with the view's actions, the view under it.
struct Stage {
    view: Entity<AgentsView>,
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (badges, actions) = self.view.update(cx, |v, cx| (v.header_badges(window, cx), v.header_actions(window, cx)));
        ui::root()
            .flex()
            .flex_col()
            .child(row().flex_none().h(px(HEADER_HEIGHT)).gap(px(SPACE_2)).children(badges).child(grow()).children(actions))
            .child(div().flex_1().min_h_0().flex().child(self.view.clone()))
    }
}

/// An open agents view.
pub(crate) struct Open {
    pub api: Arc<FixtureApi>,
    pub view: Entity<AgentsView>,
    _hub: Entity<Hub>,
}

/// The view with `selected` open, as `Route::Agents` asks for it.
pub(crate) fn open(cx: &mut TestAppContext, selected: Option<RunnableRef>) -> (Open, &mut VisualTestContext) {
    open_with(cx, selected, |_| {})
}

/// [`open`] over the fixture world changed by `edit` first.
pub(crate) fn open_with(
    cx: &mut TestAppContext,
    selected: Option<RunnableRef>,
    edit: impl FnOnce(&mut World),
) -> (Open, &mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    // After `ui::init`, which reads the preference again: popups appear and leave at once.
    ui::motion::set_reduced(true);
    let mut world = World::acme(NOW);
    edit(&mut world);
    let api = Arc::new(FixtureApi::new(world));
    let hub = cx.update(|cx| Hub::new_fixture(api.clone(), cx));
    cx.update(|cx| hub::navigate(Route::Agents { runnable: selected.clone() }, cx));
    let slot = Rc::new(RefCell::new(None));
    let put = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| AgentsView::new(selected, window, cx));
        *put.borrow_mut() = Some(view.clone());
        let stage = cx.new(|_| Stage { view });
        OverlayHost::new(window, cx).with_content(stage)
    });
    let Some(view) = slot.borrow_mut().take() else { panic!("the view was not built") };
    cx.simulate_resize(size(px(1440.), px(2400.)));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (Open { api, view, _hub: hub }, cx)
}

/// Clicks the element drawn with debug selector `selector`.
pub(crate) fn click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let bounds = cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// Double-clicks the element drawn with debug selector `selector`: two presses through the
/// window's event dispatch, the second with a click count of 2, as the platform reports one.
pub(crate) fn double_click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let at = cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out")).center();
    for click_count in [1, 2] {
        cx.simulate_event(gpui::MouseDownEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: gpui::MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent { position: at, modifiers: Modifiers::none(), button: gpui::MouseButton::Left, click_count });
        cx.run_until_parked();
    }
}

/// Whether the overlay `id` (a dialog or menu) is open.
pub(crate) fn is_open(cx: &mut VisualTestContext, id: &str) -> bool {
    let id = gpui::SharedString::from(id.to_string());
    cx.update(|window, cx| ui::overlays(window, cx).is_open(id, cx))
}

/// The bounds of the element drawn with debug selector `selector`.
pub(crate) fn bounds(cx: &mut VisualTestContext, selector: &str) -> gpui::Bounds<gpui::Pixels> {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).unwrap_or_else(|| panic!("`{selector}` was not laid out"))
}

/// An element with debug selector `selector` was drawn in the last frame.
pub(crate) fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).is_some()
}

/// `debug_bounds` wants a `'static` selector; tests leak their few.
fn leak(selector: &str) -> &'static str {
    Box::leak(selector.to_string().into_boxed_str())
}

/// The details of every recorded call of `name`.
pub(crate) fn calls(api: &FixtureApi, name: &str) -> Vec<String> {
    api.recorded().into_iter().filter(|r| r.name == name).map(|r| r.detail).collect()
}

/// The current route.
pub(crate) fn route(cx: &mut VisualTestContext) -> Route {
    cx.update(|_, cx| hub::router(cx).read(cx).current().clone())
}

/// The list's rows in order.
pub(crate) fn order(t: &Open, cx: &mut VisualTestContext) -> Vec<RunnableRef> {
    t.view.read_with(cx, |v, cx| v.order(cx))
}

/// The selection.
pub(crate) fn selected(t: &Open, cx: &mut VisualTestContext) -> Option<RunnableRef> {
    t.view.read_with(cx, |v, _| v.selected.clone())
}

pub(crate) fn lead() -> RunnableRef {
    RunnableRef::orchestrator("payments-lead")
}
