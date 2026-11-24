//! The Inbox over the fixture world, in a window, under a stand-in for the shell's header
//! (crumbs, the view's badges and actions).
//!
//! `cargo run -p screen_inbox --example preview [-- --fixture inbox-review]`
//!
//! Any scenario of `hub::fixtures::SCENARIOS` works; `empty` shows an empty inbox.

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::{Hub, Route};
use screen_inbox::InboxView;
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "inbox-review".to_string();
    while let Some(arg) = args.next() {
        if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Inbox", 1440., 900., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new(api, cx);
        let focus = match fixtures::start_route(&fixture) {
            Some((Route::Inbox { item }, _)) => item,
            _ => None,
        };
        hub::navigate(
            Route::Inbox {
                item: focus.clone(),
            },
            cx,
        );
        cx.new(|cx| Stage::new(focus, window, cx))
    });
}

/// The shell's part around the view: a header bar over the content, in an inset frame.
struct Stage {
    view: Entity<InboxView>,
}

impl Stage {
    fn new(focus: Option<hub::InboxFocus>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| InboxView::new(focus, window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        let router = hub::router(cx);
        let follow = view.clone();
        cx.observe_in(&router, window, move |_, router, window, cx| {
            if let Route::Inbox { item } = router.read(cx).current().clone() {
                follow.update(cx, |view, cx| view.show(item, window, cx));
            }
        })
        .detach();
        Self { view }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, trail, badges, actions) = self.view.update(cx, |v, cx| {
            (
                v.title(cx),
                v.trail(cx),
                v.header_badges(window, cx),
                v.header_actions(window, cx),
            )
        });
        let mut crumbs = row()
            .gap(px(SPACE_1_5))
            .type_style(HEADING)
            .child(div().text_color(TEXT_MUTED).child("Inbox"));
        for crumb in trail {
            crumbs = crumbs
                .child(icon(IconName::ChevronRight, 14., TEXT_DISABLED))
                .child(
                    div()
                        .text_color(TEXT_MUTED)
                        .font_weight(gpui::FontWeight::NORMAL)
                        .child(crumb),
                );
        }
        if title.as_ref() != "Inbox" {
            crumbs = crumbs
                .child(icon(IconName::ChevronRight, 14., TEXT_DISABLED))
                .child(title);
        }
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
                        .child(crumbs)
                        .children(badges)
                        .child(grow())
                        .child(row().flex_none().gap(px(SPACE_2)).children(actions)),
                )
                .child(div().flex_1().min_h_0().flex().child(self.view.clone())),
        )
    }
}
