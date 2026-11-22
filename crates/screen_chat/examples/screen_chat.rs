//! Standalone preview: `cargo run -p screen_chat --example screen_chat`

use gpui::AppContext as _;

fn main() {
    ui::run_app("Chat session (preview)", 1440., 960., |window, cx| {
        let view = cx.new(|cx| screen_chat::ChatScreen::new(window, cx));
        cx.new(|_| ui::Preview(view.into()))
    });
}
