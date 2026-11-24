//! The issues screen in a window over the fixture world.
//!
//! `cargo run -p screen_issues --example preview [-- --fixture issue-board|issue-comments|empty]`
//!
//! A stand-in for the shell draws the sidebar's width and the header (breadcrumb, badges,
//! actions) from the view, the way the app does.

use std::sync::Arc;

use gpui::{AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*, px};
use hub::fixtures::{FixtureApi, start_route};
use hub::{Hub, Route};
use screen_issues::IssuesView;
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "issue-board".to_string();
    while let Some(arg) = args.next() {
        if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Issues", 1440., 940., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new(api, cx);
        let route = match start_route(&fixture) {
            Some((route @ Route::Issues { .. }, _)) => route,
            _ => Route::Issues { issue: None },
        };
        hub::navigate(route, cx);
        cx.new(|cx| Stage::new(window, cx))
    });
}

/// A stand-in for the shell: the sidebar's room, then the header over the view.
struct Stage {
    view: Entity<IssuesView>,
    _router: Subscription,
}

impl Stage {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let issue = match hub::router(cx).read(cx).current() {
            Route::Issues { issue } => issue.clone(),
            _ => None,
        };
        let view = cx.new(|cx| IssuesView::new(issue, window, cx));
        let router = hub::router(cx);
        Self { view, _router: cx.observe(&router, |_, _, cx| cx.notify()) }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.view.read(cx).project_name(cx);
        let title = self.view.read(cx).title(cx);
        let (badges, actions) =
            self.view.update(cx, |view, cx| (view.header_badges(window, cx), view.header_actions(window, cx)));
        let header = row()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .gap(px(SPACE_2))
            .pl(px(SPACE_5))
            .pr(px(SPACE_2))
            .border_b_1()
            .border_color(LINE)
            .when_some(project, |d, project| {
                d.child(div().type_style(HEADING).text_color(TEXT_MUTED).child(project))
                    .child(icon(IconName::ChevronRight, 14., TEXT_MUTED))
            })
            .child(div().type_style(HEADING).child(title))
            .children(badges)
            .child(grow())
            .children(actions);
        ui::root()
            .flex()
            .flex_row()
            .child(div().flex_none().w(px(SIDEBAR_WIDTH)).h_full().bg(BG_SIDEBAR).border_r_1().border_color(LINE))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(self.view.clone())),
            )
    }
}
