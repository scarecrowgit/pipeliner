//! A chat session over the fixture world, in a window, under a stand-in for the shell's
//! header (title, the view's badges and actions).
//!
//! `cargo run -p screen_chat --example preview [-- --fixture chat-session|chat-streaming|chat-history]`
//!
//! `chat-streaming` plays the scripted reply into the transcript; `chat-history` opens the
//! same session (the palette it shows in the app belongs to the shell).

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::model::SessionId;
use hub::{Hub, Route};
use screen_chat::SessionView;
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "chat-session".to_string();
    while let Some(arg) = args.next() {
        if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Chat session", 1440., 960., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api.clone(), cx);
        let session = match fixtures::start_route(&fixture) {
            Some((Route::Chat { session }, _)) => session,
            _ => SessionId::new("chat-1423"),
        };
        hub::navigate(Route::Chat { session: session.clone() }, cx);
        api.start_scripts();
        cx.new(|cx| Stage::new(session, window, cx))
    });
}

/// The shell's part around the view: a header bar over the content.
struct Stage {
    view: Entity<SessionView>,
}

impl Stage {
    fn new(session: SessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| SessionView::new(session, window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        Self { view }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, badges, actions) = self.view.update(cx, |v, cx| {
            (v.title(cx), v.header_badges(window, cx), v.header_actions(window, cx))
        });
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
                    .child(div().text_color(TEXT_MUTED).child("Chats"))
                    .child(icon(IconName::ChevronRight, 14., TEXT_DISABLED))
                    .child(div().flex_shrink().min_w_0().child(truncate(title)))
                    .children(badges)
                    .child(grow())
                    .child(row().flex_none().gap(px(SPACE_2)).children(actions)),
            )
            .child(div().flex_1().min_h_0().flex().child(self.view.clone()))
    }
}
