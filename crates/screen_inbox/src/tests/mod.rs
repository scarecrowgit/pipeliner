//! The Inbox in a test window over the fixture backend, under a stand-in for the shell's
//! header and inside an overlay host, driven by simulated clicks and keys.

mod list;
mod markdown;
mod review;
mod select;
mod send_back;
mod texts;
mod waits;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Context, Entity, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window,
    div, prelude::*, px, size,
};
use hub::fixtures::{FixtureApi, World};
use hub::{Hub, InboxFocus, Route};
use ui::OverlayHost;
use ui::prelude::*;

use crate::InboxView;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;
/// The review of the mockups: three variants of "Double charge on retry".
pub(crate) const REVIEW: &str = "review-1423";

/// The shell's part: the header with the view's badges and actions, the view under it.
struct Stage {
    view: Entity<InboxView>,
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, badges, actions) = self.view.update(cx, |v, cx| {
            (
                v.title(cx),
                v.header_badges(window, cx),
                v.header_actions(window, cx),
            )
        });
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

/// An open Inbox.
pub(crate) struct Open {
    pub api: Arc<FixtureApi>,
    pub view: Entity<InboxView>,
    _hub: Entity<Hub>,
}

/// The Inbox opened on `focus`, over the Acme world.
pub(crate) fn open(
    cx: &mut TestAppContext,
    focus: Option<InboxFocus>,
) -> (Open, &mut VisualTestContext) {
    open_with(cx, focus, |_| {})
}

/// Like [`open`], after `edit` changed the world.
pub(crate) fn open_with(
    cx: &mut TestAppContext,
    focus: Option<InboxFocus>,
    edit: impl FnOnce(&mut World),
) -> (Open, &mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
    });
    // After `ui::init`, which reads the preference again: popups appear and leave at once.
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    api.with_world(edit);
    let hub = cx.update(|cx| Hub::new_fixture(api.clone(), cx));
    cx.update(|cx| {
        hub::navigate(
            Route::Inbox {
                item: focus.clone(),
            },
            cx,
        )
    });
    let slot = Rc::new(RefCell::new(None));
    let put = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| {
            let mut view = InboxView::new(focus, window, cx);
            view.now = Some(NOW);
            view
        });
        *put.borrow_mut() = Some(view.clone());
        let stage = cx.new(|_| Stage { view });
        OverlayHost::new(window, cx).with_content(stage)
    });
    let Some(view) = slot.borrow_mut().take() else {
        panic!("the view was not built")
    };
    cx.simulate_resize(size(px(1440.), px(1400.)));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (
        Open {
            api,
            view,
            _hub: hub,
        },
        cx,
    )
}

/// Clicks the element drawn with debug selector `selector`.
pub(crate) fn click(cx: &mut VisualTestContext, selector: &str) {
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds(leak(selector))
        .unwrap_or_else(|| panic!("`{selector}` was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

/// An element with debug selector `selector` was drawn in the last frame.
pub(crate) fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.run_until_parked();
    cx.debug_bounds(leak(selector)).is_some()
}

/// Presses keys (`"j"`, `"down"`, `"cmd-enter"`), space-separated.
pub(crate) fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
}

/// `debug_bounds` wants a `'static` selector; tests leak their few.
fn leak(selector: &str) -> &'static str {
    Box::leak(selector.to_string().into_boxed_str())
}

/// The details of every recorded call of `name`.
pub(crate) fn calls(api: &FixtureApi, name: &str) -> Vec<String> {
    api.recorded()
        .into_iter()
        .filter(|r| r.name == name)
        .map(|r| r.detail)
        .collect()
}

/// The ids of the rows of the current tab.
pub(crate) fn rows(t: &Open, cx: &mut VisualTestContext) -> Vec<String> {
    t.view.read_with(cx, |v, cx| {
        v.visible_items(cx)
            .iter()
            .map(|item| item.id.to_string())
            .collect()
    })
}

/// What is open.
pub(crate) fn focus(t: &Open, cx: &mut VisualTestContext) -> Option<InboxFocus> {
    t.view.read_with(cx, |v, _| v.focus().cloned())
}

/// The id of the open review, if one is open.
pub(crate) fn open_review(t: &Open, cx: &mut VisualTestContext) -> Option<String> {
    t.view
        .read_with(cx, |v, _| v.review.as_ref().map(|r| r.id.to_string()))
}
