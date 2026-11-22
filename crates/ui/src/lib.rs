//! Shared design system for the Harness demo: tokens, components, text input,
//! navigation and app bootstrap. Screen crates depend only on this crate.

pub mod assets;
pub mod components;
pub mod motion;
pub mod nav;
pub mod selectable;
pub mod text_input;
pub mod theme;
pub mod time;

pub use components::*;
pub use text_input::{TextInput, TextInputEvent};

use gpui::{
    AnyView, App, Application, Bounds, KeyBinding, Window, WindowBounds, WindowOptions, actions,
    div, prelude::*, px, size,
};

actions!(harness, [Quit, ToggleChatHistory]);

/// Global setup every window needs: keybindings for text inputs, quit.
pub fn init(cx: &mut App) {
    text_input::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("ctrl-k", ToggleChatHistory, None),
        KeyBinding::new("cmd-k", ToggleChatHistory, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &ToggleChatHistory, cx| {
        let open = nav::history_open(cx);
        nav::set_history_open(!open, cx);
    });
}

/// Base text style for a window's root element: Inter 13/20, text color, bg.
pub fn root() -> gpui::Div {
    div()
        .size_full()
        .bg(theme::BG)
        .text_color(theme::TEXT)
        .font_family(theme::FONT_SANS)
        .text_size(px(theme::TEXT_BODY))
        .line_height(px(theme::LINE_BODY))
}

/// Open the app (or a standalone screen preview) in a 1440×960 window.
/// `build` creates the root view.
pub fn run_app<V: Render + 'static>(
    title: &'static str,
    width: f32,
    height: f32,
    build: impl FnOnce(&mut Window, &mut App) -> gpui::Entity<V> + 'static,
) {
    Application::new().with_assets(assets::Assets).run(move |cx: &mut App| {
        init(cx);
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions { title: Some(title.into()), ..Default::default() }),
                app_id: Some("harness-demo".into()),
                window_min_size: Some(size(px(900.), px(600.))),
                ..Default::default()
            },
            move |window, cx| build(window, cx),
        )
        .expect("open window");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// Standalone preview wrapper for a screen crate's `examples/<crate>.rs`:
/// renders the screen full-window with the base root style.
pub struct Preview(pub AnyView);

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        root().flex().child(div().flex_1().min_w_0().h_full().child(self.0.clone()))
    }
}
