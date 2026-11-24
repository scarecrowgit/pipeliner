//! The session view in a test window over the fixture backend, under a stand-in for the
//! shell's header and inside an overlay host, driven by simulated clicks and keys.

mod composer;
mod header;
mod panel;
mod pickers;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, Entity, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px,
    size,
};
use hub::fixtures::{FixtureApi, World};
use hub::model::SessionId;
use hub::{Hub, Route};
use ui::OverlayHost;
use ui::prelude::*;
use ui_rich::Composer;

use super::SessionView;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;
/// The chat of the mockups: waiting on a permission, with todos, files, sub-agents and MCP.
pub(crate) const FIX: &str = "chat-1423";

/// The shell's part: the header with the view's badges and actions, the view under it.
struct Stage {
    view: Entity<SessionView>,
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

/// An open session view.
pub(crate) struct Open {
    pub api: Arc<FixtureApi>,
    pub view: Entity<SessionView>,
    pub composer: Entity<Composer>,
    _hub: Entity<Hub>,
}

pub(crate) fn open<'a>(cx: &'a mut TestAppContext, session: &str) -> (Open, &'a mut VisualTestContext) {
    open_with(cx, session, |_| {})
}

/// Like [`open`], after `edit` changed the world.
pub(crate) fn open_with<'a>(
    cx: &'a mut TestAppContext,
    session: &str,
    edit: impl FnOnce(&mut World),
) -> (Open, &'a mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    // After `ui::init`, which reads the preference again: popups appear and leave at once.
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    api.with_world(edit);
    let hub = cx.update(|cx| Hub::new(api.clone(), cx));
    let id = SessionId::new(session);
    cx.update(|cx| hub::navigate(Route::Chat { session: id.clone() }, cx));
    let slot = Rc::new(RefCell::new(None));
    let put = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| SessionView::new(id, window, cx));
        *put.borrow_mut() = Some(view.clone());
        let stage = cx.new(|_| Stage { view });
        OverlayHost::new(window, cx).with_content(stage)
    });
    let Some(view) = slot.borrow_mut().take() else { panic!("the view was not built") };
    cx.simulate_resize(size(px(1440.), px(2400.)));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let composer = view.read_with(cx, |v, _| v.composer().clone());
    (Open { api, view, composer, _hub: hub }, cx)
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

/// `debug_bounds` wants a `'static` selector; tests leak their few.
fn leak(selector: &str) -> &'static str {
    Box::leak(selector.to_string().into_boxed_str())
}

/// The details of every recorded call of `name`.
pub(crate) fn calls(api: &FixtureApi, name: &str) -> Vec<String> {
    api.recorded().into_iter().filter(|r| r.name == name).map(|r| r.detail).collect()
}

/// The composer's text.
pub(crate) fn text(t: &Open, cx: &mut VisualTestContext) -> String {
    t.composer.read_with(cx, |c, _| c.text().to_string())
}

/// Types `s` at the composer's caret, as one edit.
pub(crate) fn type_text(t: &Open, s: &str, cx: &mut VisualTestContext) {
    t.composer.update(cx, |c, cx| c.insert_text(s, cx));
}

/// Gives the composer keyboard focus.
pub(crate) fn focus_composer(t: &Open, cx: &mut VisualTestContext) {
    let composer = t.composer.clone();
    cx.update(|window, cx| composer.read(cx).focus(window, cx));
}
