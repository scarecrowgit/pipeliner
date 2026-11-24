//! The palette in a test window over the fixture backend: results, typing, chips, keys.

mod keys;
mod search;

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window, prelude::*};
use hub::fixtures::{FixtureApi, World};
use hub::{Hub, Overlay, Route};
use ui::OverlayHost;

use crate::Palette;

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;

/// The screen under the palette.
struct Stage;

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root()
    }
}

/// The overlay key of the palette's layer, as the shell names it.
const LAYER: &str = "palette";

/// Open `palette` in a dialog layer, as the shell does: the layer closing on its own tells the
/// router, and the router closing the overlay closes the layer.
fn mount(palette: Entity<Palette>, window: &mut Window, cx: &mut gpui::App) {
    let router = hub::router(cx);
    let follow = router.clone();
    ui::overlays(window, cx).open_dialog_view(
        LAYER,
        palette,
        move |_, cx| {
            if follow.read(cx).is_open(Overlay::Palette) {
                follow.update(cx, |router, cx| router.close_overlay(Overlay::Palette, cx));
            }
        },
        window,
        cx,
    );
    window
        .observe(&router, cx, |router, window, cx| {
            if !router.read(cx).is_open(Overlay::Palette) {
                ui::overlays(window, cx).close(LAYER, window, cx);
            }
        })
        .detach();
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
    let (_host, cx) = cx.add_window_view(|window, cx| {
        let stage = cx.new(|_| Stage);
        OverlayHost::new(window, cx).with_content(stage)
    });
    cx.update(|window, _| window.activate_window());
    let palette = cx.update(|window, cx| {
        let palette = cx.new(|cx| Palette::new(&hub, window, cx));
        mount(palette.clone(), window, cx);
        palette
    });
    cx.run_until_parked();
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
