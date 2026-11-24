//! The issue panel alone over the fixture world, in a 400px frame on the right of a window as
//! the board mounts it.
//!
//! `cargo run -p screen_issues --example panel [-- --fixture issue-comments|issue-board] [--issue PAY-1427] [--height 1400]`
//!
//! `issue-board` opens PAY-1440 (nothing attached), `issue-comments` PAY-1427 (PAY triage
//! running, a conversation with mentions). Closing the panel brings it back after a moment.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::model::IssueKey;
use hub::{Hub, Route};
use screen_issues::{IssuePanel, PanelEvent};
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "issue-comments".to_string();
    let mut issue = None;
    let mut height = 960.;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => fixture = args.next().unwrap_or(fixture),
            "--issue" => issue = args.next().map(IssueKey::new),
            "--height" => height = args.next().and_then(|h| h.parse().ok()).unwrap_or(height),
            _ => {}
        }
    }
    ui::run_app("Issue panel", 1440., height, move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api, cx);
        let start = fixtures::start_route(&fixture).map(|(route, _)| route);
        let key = issue.clone().or(match start {
            Some(Route::Issues { issue }) => issue,
            _ => None,
        });
        let key = key.unwrap_or_else(|| IssueKey::new("PAY-1427"));
        hub::navigate(Route::Issues { issue: Some(key.clone()) }, cx);
        cx.new(|cx| Stage::new(key, window, cx))
    });
}

/// The board's part: empty ground on the left, the panel on the right.
struct Stage {
    key: IssueKey,
    panel: Option<Entity<IssuePanel>>,
    _sub: Option<Subscription>,
}

impl Stage {
    fn new(key: IssueKey, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut stage = Self { key, panel: None, _sub: None };
        stage.open(window, cx);
        stage
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.new(|cx| IssuePanel::new(self.key.clone(), window, cx));
        self._sub = Some(cx.subscribe_in(&panel, window, |this, _, event: &PanelEvent, window, cx| match event {
            PanelEvent::Close => {
                this.panel = None;
                cx.notify();
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(800)).await;
                    this.update_in(cx, |this, window, cx| {
                        this.open(window, cx);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }));
        self.panel = Some(panel);
    }
}

impl Render for Stage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .flex()
            .flex_row()
            .child(div().flex_1().min_w_0().h_full().bg(BG))
            .when_some(self.panel.clone(), |d, panel| {
                // The board's frame: 400 wide with a rule on its left.
                d.child(div().flex_none().w(px(400.)).h_full().border_l_1().border_color(LINE).child(panel))
            })
    }
}
