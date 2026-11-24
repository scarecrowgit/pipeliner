//! The transcript of one session over the fixture world, alone in a window.
//!
//! `cargo run -p screen_chat --example transcript [-- --fixture chat-streaming] [--session chat-ledger]`
//!
//! `chat-session` shows the mockup's chat waiting on its permission question, `chat-streaming`
//! plays the rest of that turn as it streams, `chat-history` opens the same chat; pass
//! `--session chat-ledger` for a long chat that pages older messages in.

use std::sync::Arc;

use gpui::{AppContext as _, Entity};
use hub::fixtures::{self, FixtureApi};
use hub::model::SessionId;
use hub::{Hub, Route};
use screen_chat::TranscriptView;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "chat-session".to_string();
    let mut session: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => fixture = args.next().unwrap_or(fixture),
            "--session" => session = args.next(),
            _ => {}
        }
    }
    ui::run_app("Transcript", 960., 900., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        let _hub: Entity<Hub> = Hub::new(api.clone(), cx);
        let id = match (session, fixtures::start_route(&fixture)) {
            (Some(id), _) => SessionId::new(&id),
            (None, Some((Route::Chat { session }, _))) => session,
            (None, _) => SessionId::new("chat-1423"),
        };
        let view = cx.new(|cx| TranscriptView::new(id, window, cx));
        api.start_scripts();
        cx.new(|_| ui::Preview(view.into()))
    });
}
