//! The window's chrome: sidebar or rail, the inset content frame with the header bar and the
//! current screen, the palette slot, and hub notices as toasts.

use std::collections::HashMap;

use std::rc::Rc;

use gpui::{AnyView, App, Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*, px};
use hub::{HubNotice, Overlay, Route, Router};
use ui::prelude::*;

use crate::cache::{CacheKey, ScreenCache};
use crate::header::HeaderBar;
use crate::placeholder;
use crate::screen::{Header, Mounted, ScreenKind, Screens, section_label};
use crate::sidebar::Sidebar;

/// The overlay key of the palette's dialog layer.
pub const PALETTE_LAYER: &str = "palette";

/// Builds a fresh palette view each time the palette opens.
type PaletteFactory = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// The screen on show.
struct Current {
    route: Route,
    mounted: Mounted,
    placeholder: bool,
}

/// The shell view. Make it the window's content with [`crate::mount`], which puts the
/// overlay host around it.
pub struct Shell {
    screens: Screens,
    cache: ScreenCache<Mounted>,
    placeholders: HashMap<ScreenKind, Mounted>,
    current: Option<Current>,
    sidebar: Entity<Sidebar>,
    router: Entity<Router>,
    palette: Option<PaletteFactory>,
    mounts: usize,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// A shell showing the router's current route with the screens of `screens`. Needs
    /// [`hub::Hub::new`] first.
    pub fn new(screens: Screens, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = hub::hub(cx);
        let router = hub::router(cx);
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let subscriptions = vec![
            cx.observe_in(&router, window, |this: &mut Shell, _, window, cx| this.sync(window, cx)),
            cx.subscribe_in(&hub, window, |_, _, notice: &HubNotice, window, cx| {
                ui::overlays(window, cx).toast(crate::notice::toast_for(notice), cx);
            }),
        ];
        let mut shell = Shell {
            screens,
            cache: ScreenCache::default(),
            placeholders: HashMap::new(),
            current: None,
            sidebar,
            router,
            palette: None,
            mounts: 0,
            _subscriptions: subscriptions,
        };
        let route = shell.router.read(cx).current().clone();
        shell.show(route, window, cx);
        // The overlay host is not reachable until the window's root exists.
        cx.defer_in(window, |this, window, cx| this.sync(window, cx));
        shell
    }

    /// Mount the palette: `build` makes the view shown in a dialog layer each time the router
    /// opens [`Overlay::Palette`], so every opening starts afresh. The view positions itself
    /// in the layer and must occlude what is under it; the layer gives it the scrim, the fade
    /// and Escape. When the layer closes on its own (Escape, a press on the scrim) the router
    /// hears of it.
    pub fn set_palette(
        &mut self,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette = Some(Rc::new(build));
        self.sync(window, cx);
    }

    /// The route on show.
    pub fn route(&self) -> Option<&Route> {
        self.current.as_ref().map(|current| &current.route)
    }

    /// The view of the screen on show.
    pub fn body(&self) -> Option<&AnyView> {
        self.current.as_ref().map(|current| current.mounted.body())
    }

    /// The screen on show is the placeholder of an unregistered kind.
    pub fn showing_placeholder(&self) -> bool {
        self.current.as_ref().is_some_and(|current| current.placeholder)
    }

    /// How many times a registered screen was built.
    pub fn mounts(&self) -> usize {
        self.mounts
    }

    /// Registered screen views kept alive now.
    pub fn kept_views(&self) -> usize {
        self.cache.len()
    }

    /// The sidebar view.
    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    /// Follow the router: the screen for its route, and the palette layer for its overlay.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let route = self.router.read(cx).current().clone();
        if self.route() != Some(&route) {
            self.show(route, window, cx);
        }
        let wanted = self.router.read(cx).is_open(Overlay::Palette);
        let overlays = ui::overlays(window, cx);
        if !overlays.is_mounted(cx) {
            return;
        }
        let open = overlays.is_open(PALETTE_LAYER, cx);
        match (wanted, open, self.palette.clone()) {
            (true, false, Some(build)) => {
                let view = build(window, cx);
                let router = self.router.clone();
                overlays.open_dialog_view(
                    PALETTE_LAYER,
                    view,
                    move |_, cx| {
                        if router.read(cx).is_open(Overlay::Palette) {
                            router.update(cx, |router, cx| router.close_overlay(Overlay::Palette, cx));
                        }
                    },
                    window,
                    cx,
                );
            }
            // Nothing to show: do not leave the router thinking the palette is up.
            (true, false, None) => {
                let router = self.router.clone();
                cx.defer_in(window, move |_, _, cx| {
                    router.update(cx, |router, cx| router.close_overlay(Overlay::Palette, cx));
                });
            }
            (false, true, _) => overlays.close(PALETTE_LAYER, window, cx),
            _ => {}
        }
    }

    /// Show the screen of `route`: a kept view when there is one, else a new one.
    fn show(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        let kind = ScreenKind::of(&route);
        let (mounted, placeholder) = match self.screens.get(kind).cloned() {
            Some(screen) => {
                let keep = screen.keep();
                let key = CacheKey::new(&route, keep);
                match self.cache.touch(&key).cloned() {
                    Some(mounted) => (mounted, false),
                    None => {
                        let mounted = screen.mount(&route, window, cx);
                        self.mounts += 1;
                        self.cache.insert(key, mounted.clone(), keep);
                        (mounted, false)
                    }
                }
            }
            None => {
                let mounted = self
                    .placeholders
                    .entry(kind)
                    .or_insert_with(|| placeholder::mounted(kind, &route, cx))
                    .clone();
                (mounted, true)
            }
        };
        if let Some(on_show) = mounted.on_show.clone() {
            on_show(&route, window, cx);
        }
        self.current = Some(Current { route, mounted, placeholder });
        cx.notify();
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.current.as_ref().map(|current| {
            let fallback = section_label(current.route.section());
            match current.mounted.header.clone() {
                Some(f) => f(window, cx),
                None => Header::title(fallback),
            }
        });
        let body = self.current.as_ref().map(|current| current.mounted.body.clone());
        let frame = col()
            .id("content-frame")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .my(px(SPACE_2))
            .mr(px(SPACE_2))
            .bg(BG)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .overflow_hidden()
            .relative()
            .children(header.filter(|header| !header.hidden).map(HeaderBar::new))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .children(body.map(|body| div().flex().flex_1().min_w_0().h_full().child(body))),
            );
        ui::root().bg(BG_SIDEBAR).flex().flex_row().child(self.sidebar.clone()).child(frame)
    }
}
