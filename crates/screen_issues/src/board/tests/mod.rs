//! The issues screen in a test window over the fixture backend.

mod autopilot;
mod board;
mod dialog;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
use hub::fixtures::{FixtureApi, World};
use hub::model::IssueKey;
use hub::{Hub, Route};
use ui::OverlayHost;

use super::IssuesView;

/// The fixed clock of the test world.
const NOW: i64 = 1_795_000_000_000;

/// A window over the Acme world showing the issues screen at `issue`.
fn open<'a>(cx: &'a mut TestAppContext, issue: Option<&str>) -> (Arc<FixtureApi>, Entity<IssuesView>, &'a mut VisualTestContext) {
    open_with(cx, issue, |_| {})
}

/// [`open`] over a world changed by `change` first.
fn open_with<'a>(
    cx: &'a mut TestAppContext,
    issue: Option<&str>,
    change: impl FnOnce(&mut World),
) -> (Arc<FixtureApi>, Entity<IssuesView>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    ui::motion::set_reduced(true);
    let mut world = World::acme(NOW);
    change(&mut world);
    let api = Arc::new(FixtureApi::new(world));
    let issue = issue.map(IssueKey::new);
    let route = Route::Issues { issue: issue.clone() };
    cx.update(|cx| {
        Hub::new(api.clone(), cx);
        hub::navigate(route, cx);
    });
    let slot: Rc<RefCell<Option<Entity<IssuesView>>>> = Rc::default();
    let put = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| IssuesView::new(issue, window, cx));
        *put.borrow_mut() = Some(view.clone());
        OverlayHost::new(window, cx).with_content(view)
    });
    let Some(view) = slot.borrow_mut().take() else { panic!("the view was not built") };
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (api, view, cx)
}

/// Clicks the element drawn with debug selector `selector`.
fn click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let name: &'static str = Box::leak(selector.to_string().into_boxed_str());
    let Some(bounds) = cx.debug_bounds(name) else { panic!("`{selector}` was not laid out") };
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// Whether `selector` is drawn.
fn drawn(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.run_until_parked();
    let name: &'static str = Box::leak(selector.to_string().into_boxed_str());
    cx.debug_bounds(name).is_some()
}

fn route(cx: &mut VisualTestContext) -> Route {
    cx.update(|_, cx| hub::router(cx).read(cx).current().clone())
}

/// The command names the fixture received, `name detail` each.
fn recorded(api: &FixtureApi) -> Vec<String> {
    api.recorded().into_iter().map(|c| format!("{} {}", c.name, c.detail)).collect()
}

/// Opens the new-issue dialog as the header's button does (the shell draws the header).
fn new_issue(view: &Entity<IssuesView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| view.update(cx, |view, cx| view.new_issue(None, window, cx)));
    cx.run_until_parked();
}

/// Opens the autopilots popover as the header's button does.
fn autopilots(view: &Entity<IssuesView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| view.update(cx, |view, cx| view.open_autopilots(window, cx)));
    cx.run_until_parked();
}
