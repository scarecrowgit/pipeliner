//! The editor in a test window over the fixture backend, under a stand-in for the shell's
//! header and inside an overlay host, driven by simulated clicks, keys and the clock.

mod deploy;
mod dock;
mod load_save;
mod narrow;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, Entity, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px,
    size,
};
use hub::api::GraphDef;
use hub::fixtures::{FixtureApi, World};
use hub::model::{NodeId, PipelineId};
use hub::{Hub, Route};
use ui::OverlayHost;
use ui::prelude::*;

use super::load_save::{CHECK_DELAY, SAVE_DELAY};
use crate::EditorView;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;
/// The pipeline of the mockups.
pub(crate) const PAY: &str = "pay-triage";

/// The shell's part: the header with the view's badges and actions, the view under it.
struct Stage {
    view: Entity<EditorView>,
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, badges, actions) =
            self.view.update(cx, |v, cx| (v.title(cx), v.header_badges(window, cx), v.header_actions(window, cx)));
        // As the shell lays it out: the badges are clipped before the title, the actions stay.
        let mut after = row().gap(px(SPACE_2)).min_w_0().overflow_hidden();
        after.style().flex_shrink = Some(1000.);
        let header = row()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .gap(px(SPACE_2))
            .pl(px(SPACE_5))
            .pr(px(SPACE_2))
            .child(row().min_w_0().flex_shrink().child(truncate(title)))
            .child(after.children(badges.into_iter().map(|badge| div().flex_none().child(badge))))
            .child(grow())
            .child(row().flex_none().gap(px(SPACE_2)).children(actions));
        ui::root()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(self.view.clone()))
    }
}

/// An open editor.
pub(crate) struct Open {
    pub api: Arc<FixtureApi>,
    pub view: Entity<EditorView>,
    _hub: Entity<Hub>,
}

/// The editor opened on PAY triage with the Fixer selected, over the Acme world.
pub(crate) fn open(cx: &mut TestAppContext) -> (Open, &mut VisualTestContext) {
    open_with(cx, |_| {})
}

/// Like [`open`], after `edit` changed the world.
pub(crate) fn open_with(cx: &mut TestAppContext, edit: impl FnOnce(&mut World)) -> (Open, &mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    api.with_world(edit);
    let hub = cx.update(|cx| Hub::new_fixture(api.clone(), cx));
    let route = Route::Editor { pipeline: PipelineId::new(PAY), node: Some(NodeId::new("fixer")) };
    cx.update(|cx| hub::navigate(route, cx));
    let slot = Rc::new(RefCell::new(None));
    let put = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| EditorView::new(PipelineId::new(PAY), Some(NodeId::new("fixer")), window, cx));
        *put.borrow_mut() = Some(view.clone());
        let stage = cx.new(|_| Stage { view });
        OverlayHost::new(window, cx).with_content(stage)
    });
    let Some(view) = slot.borrow_mut().take() else { panic!("the view was not built") };
    cx.simulate_resize(size(px(1440.), px(1060.)));
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

/// An element with debug selector `selector` was drawn in the last frame.
pub(crate) fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).is_some()
}

/// Lets `delay` pass on the test clock and runs what became due.
pub(crate) fn wait(cx: &mut VisualTestContext, delay: std::time::Duration) {
    cx.executor().advance_clock(delay);
    cx.run_until_parked();
}

/// Lets the save debounce pass.
pub(crate) fn wait_save(cx: &mut VisualTestContext) {
    wait(cx, SAVE_DELAY + CHECK_DELAY);
}

/// Lets the check debounce pass.
pub(crate) fn wait_check(cx: &mut VisualTestContext) {
    wait(cx, CHECK_DELAY);
}

fn leak(selector: &str) -> &'static str {
    Box::leak(selector.to_string().into_boxed_str())
}

/// The details of every recorded call of `name`.
pub(crate) fn calls(api: &FixtureApi, name: &str) -> Vec<String> {
    api.recorded().into_iter().filter(|r| r.name == name).map(|r| r.detail).collect()
}

/// Edits the draft through the view, as the canvas or inspector would.
pub(crate) fn edit(t: &Open, cx: &mut VisualTestContext, what: &'static str, change: impl FnOnce(&mut GraphDef)) {
    let draft = t.view.read_with(cx, |v, _| v.draft().clone());
    draft.update(cx, |draft, cx| draft.edit(what, change, cx));
    cx.run_until_parked();
}

/// The draft's graph.
pub(crate) fn graph(t: &Open, cx: &mut VisualTestContext) -> GraphDef {
    t.view.read_with(cx, |v, cx| v.draft().read(cx).graph().cloned().expect("the draft is loaded"))
}

/// The fixture's file of PAY triage.
pub(crate) fn on_disk(t: &Open) -> GraphDef {
    t.api.with_world(|w| w.pipelines.iter().find(|g| g.id.as_str() == PAY).cloned().expect("PAY triage exists"))
}
