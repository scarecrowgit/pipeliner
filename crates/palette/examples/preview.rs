//! The palette open over the fixture world, in a window.
//!
//! `cargo run -p palette --example preview [-- --fixture chat-history]`
//!
//! Any scenario of `hub::fixtures::SCENARIOS` works (`empty` shows the empty state). Cmd/Ctrl-K
//! closes and reopens the palette, as the shell's shortcut does.

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, KeyBinding, Render, Subscription, Window, actions, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::{Hub, Overlay, Route};
use palette::Palette;
use ui::theme::*;

actions!(preview, [TogglePalette]);

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "chat-history".to_string();
    while let Some(arg) = args.next() {
        if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Palette", 1440., 960., move |window, cx| {
        ui_rich::bind_keys(cx);
        palette::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-k", TogglePalette, None), KeyBinding::new("ctrl-k", TogglePalette, None)]);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        let hub = Hub::new(api, cx);
        let (route, overlay) = fixtures::start_route(&fixture).unwrap_or((Route::Home, Some(Overlay::Palette)));
        hub::navigate(route, cx);
        if let Some(overlay) = overlay.or(Some(Overlay::Palette)) {
            hub::router(cx).update(cx, |router, cx| router.open_overlay(overlay, cx));
        }
        cx.new(|cx| Stage::new(hub, window, cx))
    });
}

/// A stand-in for the app behind the palette, and the shell's mounting logic: a fresh palette
/// in a dialog layer each time the router opens it.
struct Stage {
    hub: Entity<Hub>,
    _router: Subscription,
}

impl Stage {
    fn new(hub: Entity<Hub>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let router = hub::router(cx);
        let subscription = cx.observe_in(&router, window, |this, router, window, cx| this.follow(&router, window, cx));
        // The overlay host exists once this root is built.
        cx.defer_in(window, move |this, window, cx| this.follow(&router, window, cx));
        Self { hub, _router: subscription }
    }

    fn follow(&mut self, router: &Entity<hub::Router>, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = router.read(cx).is_open(Overlay::Palette);
        let overlays = ui::overlays(window, cx);
        match (wanted, overlays.is_open(LAYER, cx)) {
            (true, false) => {
                let palette = cx.new(|cx| Palette::new(&self.hub, window, cx));
                let router = router.clone();
                overlays.open_dialog_view(
                    LAYER,
                    palette,
                    move |_, cx| {
                        if router.read(cx).is_open(Overlay::Palette) {
                            router.update(cx, |router, cx| router.close_overlay(Overlay::Palette, cx));
                        }
                    },
                    window,
                    cx,
                );
            }
            (false, true) => overlays.close(LAYER, window, cx),
            _ => {}
        }
    }
}

/// The overlay key of the palette's layer.
const LAYER: &str = "palette";

impl Render for Stage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .relative()
            .on_action(cx.listener(|_, _: &TogglePalette, _, cx| {
                hub::router(cx).update(cx, |router, cx| router.toggle_overlay(Overlay::Palette, cx));
            }))
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(TEXT_MUTED)
                    .child(div().p(px(SPACE_4)).child("Cmd/Ctrl-K opens the palette")),
            )
    }
}

