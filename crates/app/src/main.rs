//! Pipeliner desktop app: a GPUI shell over the engine and its store.

mod assets;
mod screens;
mod shell;
mod state;
mod theme;
mod ui;

use gpui::{
    px, size, App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds,
    WindowOptions,
};
use gpui_component::Root;
use harness_core::{Config, Store};
use harness_engine::Engine;

use crate::state::{AppState, ToastKind};

fn main() {
    Application::new().with_assets(assets::Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        theme::init_fonts(cx);
        theme::apply_component_theme(cx);
        shell::init(cx);

        // Store: fall back to an in-memory database (and say so) rather than not starting.
        let (store, store_error) = match Store::open_default() {
            Ok(s) => (s, None),
            Err(e) => {
                eprintln!("pipeliner: cannot open the database: {e:#}");
                let s = Store::open_in_memory().expect("in-memory sqlite");
                (s, Some(format!("Could not open the database ({e:#}). Changes this session are not saved.")))
            }
        };
        let config = Config::load();
        let engine = Engine::new(store.clone(), config);
        // PIPELINER_STILL: show the data as it is (screenshots of a fixture). The engine
        // doesn't take over, so nothing is recovered, resumed or run.
        let still = std::env::var_os("PIPELINER_STILL").is_some();
        if !still {
            engine.start();
        }
        store.watch_external();

        let app_state = AppState::init(engine, cx);
        // PIPELINER_ROUTE: open on a screen, e.g. `review:<task id>` (see `Route::parse`).
        if let Some(route) = std::env::var("PIPELINER_ROUTE")
            .ok()
            .and_then(|r| state::Route::parse(&r))
        {
            state::replace_route(route, cx);
        }
        if let Some(msg) = store_error {
            app_state.update(cx, |s, cx| s.toast(ToastKind::Danger, msg, cx));
        }

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1100.), px(700.))),
            titlebar: Some(TitlebarOptions { title: Some("Pipeliner".into()), ..Default::default() }),
            app_id: Some("pipeliner".into()),
            focus: true,
            show: true,
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            window.set_window_title("Pipeliner");
            let shell = cx.new(|cx| shell::Shell::new(window, cx));
            cx.new(|cx| Root::new(shell, window, cx))
        })
        .expect("open the main window");
        cx.activate(true);
    });
}
