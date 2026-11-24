//! The new-chat page, or the list of every chat, in a window over the fixture world.
//!
//! `cargo run -p screen_chat --example new_chat [-- --fixture new-chat] [--page chats]`
//!
//! Any scenario of `hub::fixtures::SCENARIOS` works (`empty` shows a fresh install). Starting a
//! chat or opening one shows a stand-in for the session view, with a way back.

use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use hub::fixtures::FixtureApi;
use hub::{Hub, Route};
use screen_chat::{ChatsView, NewChatView};
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "new-chat".to_string();
    let mut chats = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => {
                if let Some(name) = args.next() {
                    fixture = name;
                }
            }
            "--page" => chats = args.next().as_deref() == Some("chats"),
            _ => {}
        }
    }
    ui::run_app("New chat", 1440., 900., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new(api, cx);
        hub::navigate(if chats { Route::Chats } else { Route::NewChat }, cx);
        cx.new(|cx| Stage::new(window, cx))
    });
}

/// A stand-in for the shell: a header bar over whichever of the two views the route shows.
struct Stage {
    new_chat: Entity<NewChatView>,
    chats: Entity<ChatsView>,
    _router: Subscription,
}

impl Stage {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let router = hub::router(cx);
        Self {
            new_chat: cx.new(|cx| NewChatView::new(window, cx)),
            chats: cx.new(|cx| ChatsView::new(window, cx)),
            _router: cx.observe(&router, |_, _, cx| cx.notify()),
        }
    }

    fn header(title: SharedString, after: Vec<AnyElement>, actions: Vec<AnyElement>) -> impl IntoElement {
        row()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .gap(px(SPACE_2))
            .pl(px(SPACE_5))
            .pr(px(SPACE_2))
            .border_b_1()
            .border_color(LINE)
            .child(div().type_style(HEADING).child(title))
            .children(after)
            .child(grow())
            .children(actions)
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let route = hub::router(cx).read(cx).current().clone();
        let (header, body): (AnyElement, AnyElement) = match route {
            Route::Chats => {
                let title = self.chats.read(cx).title(cx);
                let actions = self.chats.update(cx, |view, cx| view.header_actions(window, cx));
                (Self::header(title, Vec::new(), actions).into_any_element(), self.chats.clone().into_any_element())
            }
            Route::Chat { session } => (
                Self::header("Chat".into(), Vec::new(), Vec::new()).into_any_element(),
                col()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(SPACE_3))
                    .text_color(TEXT_MUTED)
                    .child(format!("Opened {session}: the session view is not part of this preview."))
                    .child(
                        row()
                            .gap(px(SPACE_2))
                            .child(
                                Button::new("back-new", "New chat")
                                    .secondary()
                                    .on_click(|_, _, cx| hub::navigate(Route::NewChat, cx)),
                            )
                            .child(Button::new("back-chats", "All chats").ghost().on_click(|_, _, cx| hub::navigate(Route::Chats, cx))),
                    )
                    .into_any_element(),
            ),
            _ => {
                let title = self.new_chat.read(cx).title(cx);
                let badges = self.new_chat.read(cx).header_badges(cx);
                let actions = self.new_chat.update(cx, |view, cx| view.header_actions(window, cx));
                (Self::header(title, badges, actions).into_any_element(), self.new_chat.clone().into_any_element())
            }
        };
        ui::root().flex().flex_col().child(header).child(div().flex_1().min_h_0().child(body))
    }
}
