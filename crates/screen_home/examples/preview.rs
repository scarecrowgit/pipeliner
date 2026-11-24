//! Home over the fixture world, in a window, under a stand-in for the shell's header.
//!
//! `cargo run -p screen_home --example preview [-- --fixture home]`
//!
//! Any scenario of `hub::fixtures::SCENARIOS` works; `empty` shows the empty states.

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::{Hub, Route};
use screen_home::HomeView;
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "home".to_string();
    while let Some(arg) = args.next() {
        if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Home", 1440., 960., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api, cx);
        let route = fixtures::start_route(&fixture).map_or(Route::Home, |(route, _)| route);
        hub::navigate(route, cx);
        cx.new(|cx| Stage::new(window, cx))
    });
}

/// The shell's part around the view: a header bar over the content, in an inset frame.
struct Stage {
    view: Entity<HomeView>,
}

impl Stage {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| HomeView::new(window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        Self { view }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, badges, actions) =
            self.view.update(cx, |v, cx| (v.title(cx), v.header_badges(window, cx), v.header_actions(window, cx)));
        ui::root().bg(BG_SIDEBAR).flex().p(px(SPACE_2)).child(
            col()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(BG)
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .overflow_hidden()
                .child(
                    row()
                        .flex_none()
                        .h(px(HEADER_HEIGHT))
                        .gap(px(SPACE_2))
                        .pl(px(SPACE_5))
                        .pr(px(SPACE_2))
                        .border_b_1()
                        .border_color(LINE)
                        .child(div().type_style(HEADING).child(title))
                        .children(badges)
                        .child(grow())
                        .child(row().flex_none().gap(px(SPACE_2)).children(actions)),
                )
                .child(div().flex_1().min_h_0().flex().child(self.view.clone())),
        )
    }
}
