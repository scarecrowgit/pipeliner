//! The palette in a test window over the fixture backend: results, typing, chips, keys.

mod keys;
mod search;

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window, prelude::*};
use hub::fixtures::{FixtureApi, World};
use hub::{Hub, Overlay, Route};

use crate::Palette;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;

/// A window root holding the palette, as the shell does.
struct Stage {
    palette: Entity<Palette>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().relative().child(self.palette.clone())
    }
}

/// An open palette over the Acme world, with the search settled.
pub(crate) struct Open {
    pub api: Arc<FixtureApi>,
    pub palette: Entity<Palette>,
}

pub(crate) fn open(cx: &mut TestAppContext) -> (Open, &mut VisualTestContext) {
    cx.update(|cx| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
        crate::init(cx);
    });
    // After `ui::init`, which reads the preference again: popups appear and leave at once.
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    let hub = cx.update(|cx| Hub::new(api.clone(), cx));
    cx.update(|cx| hub::navigate(Route::Home, cx));
    cx.update(|cx| hub::router(cx).update(cx, |router, cx| router.open_overlay(Overlay::Palette, cx)));
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let put = slot.clone();
    let h = hub.clone();
    let (_stage, cx) = cx.add_window_view(move |window, cx| {
        let palette = cx.new(|cx| Palette::new(&h, window, cx));
        *put.borrow_mut() = Some(palette.clone());
        Stage { palette }
    });
    let Some(palette) = slot.borrow_mut().take() else { panic!("the palette was not built") };
    cx.update(|window, _| window.activate_window());
    palette.update(cx, |palette, cx| palette.set_now(NOW, cx));
    settle(cx);
    (Open { api, palette }, cx)
}

/// Lets a pending debounce run and the results arrive.
pub(crate) fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(hub::models::DEBOUNCE * 3);
    cx.run_until_parked();
}

/// The titles of the shown results, in row order.
pub(crate) fn titles(palette: &Entity<Palette>, cx: &mut VisualTestContext) -> Vec<String> {
    palette.read_with(cx, |palette, _| {
        let hits = palette.result.as_ref().map(|result| result.hits.clone()).unwrap_or_default();
        palette.listing.rows.iter().filter_map(|&hit| hits.get(hit)).map(|hit| hit.session.title.clone()).collect()
    })
}

pub(crate) fn overlay_open(cx: &mut VisualTestContext) -> bool {
    cx.update(|_, cx| hub::router(cx).read(cx).is_open(Overlay::Palette))
}

pub(crate) fn route(cx: &mut VisualTestContext) -> Route {
    cx.update(|_, cx| hub::router(cx).read(cx).current().clone())
}
