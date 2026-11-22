//! Standalone preview: `cargo run -p screen_review --example screen_review`

use gpui::AppContext as _;

fn main() {
    ui::run_app("Review queue (preview)", 1440., 960., |window, cx| {
        let view = cx.new(|cx| screen_review::ReviewScreen::new(window, cx));
        cx.new(|_| ui::Preview(view.into()))
    });
}
