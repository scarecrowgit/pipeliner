//! The agents screen over the fixture world, in a window, under a stand-in for the shell's
//! header (title, the view's badges and actions).
//!
//! `cargo run -p screen_agents --example preview [-- --fixture agents|empty]`

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::{Hub, Route};
use screen_agents::AgentsView;
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "agents".to_string();
    while let Some(arg) = args.next() {
        if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Agents & pipelines", 1440., 960., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api, cx);
        let route = match fixtures::start_route(&fixture) {
            Some((route @ Route::Agents { .. }, _)) => route,
            _ => Route::Agents { runnable: None },
        };
        let selected = match &route {
            Route::Agents { runnable } => runnable.clone(),
            _ => None,
        };
        hub::navigate(route, cx);
        cx.new(|cx| Stage::new(selected, window, cx))
    });
}

/// The shell's part around the view: a header bar over the content.
struct Stage {
    view: Entity<AgentsView>,
}

impl Stage {
    fn new(selected: Option<hub::model::RunnableRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| AgentsView::new(selected, window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        Self { view }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, badges, actions) =
            self.view.update(cx, |v, cx| (v.title(cx), v.header_badges(window, cx), v.header_actions(window, cx)));
        ui::root()
            .flex()
            .flex_col()
            .child(
                row()
                    .flex_none()
                    .h(px(HEADER_HEIGHT))
                    .gap(px(SPACE_2))
                    .pl(px(SPACE_5))
                    .pr(px(SPACE_2))
                    .border_b_1()
                    .border_color(LINE)
                    .type_style(HEADING)
                    .child(div().flex_none().child(title))
                    .children(badges)
                    .child(grow())
                    .child(row().flex_none().gap(px(SPACE_2)).children(actions)),
            )
            .child(div().flex_1().min_h_0().flex().child(self.view.clone()))
    }
}
