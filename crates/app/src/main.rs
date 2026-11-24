//! The harness desktop app: wires the engine, the hub, the shell and the screens.
//!
//! `harness` runs over the real engine with the user's config and data.
//! `harness --fixture <name>` runs over the in-memory fixture backend instead, for previews
//! and for scripted checks through the dev driver (`HARNESS_DEV_SOCKET`).

mod screens;

use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use hub::api::Api;
use hub::fixtures::{FixtureApi, SCENARIOS, start_route};
use hub::{Hub, Overlay};

/// What the command line asked for.
struct Args {
    /// Run over the fixture backend with this scenario instead of the real engine.
    fixture: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut fixture = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--fixture" {
            fixture = Some(args.next().ok_or_else(|| anyhow!("--fixture needs a name"))?);
        } else if let Some(value) = arg.strip_prefix("--fixture=") {
            fixture = Some(value.to_string());
        } else {
            return Err(anyhow!("unknown argument `{arg}`; usage: harness [--fixture <name>]"));
        }
    }
    Ok(Args { fixture })
}

/// The backend the window runs over, and the fixture to start when there is one.
fn backend(args: &Args) -> Result<(Arc<dyn Api>, Option<Arc<FixtureApi>>)> {
    match &args.fixture {
        Some(name) => {
            let api = FixtureApi::scenario(name)
                .ok_or_else(|| anyhow!("unknown fixture `{name}`; one of: {}", SCENARIOS.join(", ")))?;
            let api = Arc::new(api);
            Ok((api.clone(), Some(api)))
        }
        None => {
            let engine = engine::Engine::open_configured(base::paths::Paths::from_env())
                .context("could not open the harness data directory")?;
            Ok((Arc::new(engine), None))
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(2);
        }
    };
    let (api, fixture) = match backend(&args) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    };

    Application::new().with_assets(ui::assets::Assets).run(move |cx: &mut App| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
        shell::init(cx);
        palette::init(cx);
        Hub::new(api.clone(), cx);
        if let Some((route, overlay)) = args.fixture.as_deref().and_then(start_route) {
            hub::navigate(route, cx);
            if let Some(overlay) = overlay {
                hub::router(cx).update(cx, |router, cx| router.open_overlay(overlay, cx));
            }
        }

        let bounds = Bounds::centered(None, size(px(1440.), px(960.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Harness".into()), ..Default::default() }),
            app_id: Some("harness".into()),
            window_min_size: Some(size(px(900.), px(600.))),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            let (shell, root) = shell::mount(screens::all(), window, cx);
            // The palette is built anew each time it opens: it takes focus and starts from an
            // empty query on creation.
            let router = hub::router(cx);
            let hub = hub::hub(cx);
            let mut was_open = false;
            let slot = shell.downgrade();
            window
                .observe(&router, cx, move |router, window, cx| {
                    let open = router.read(cx).is_open(Overlay::Palette);
                    if open && !was_open {
                        let view = cx.new(|cx| palette::Palette::new(&hub, window, cx));
                        slot.update(cx, |shell, cx| shell.set_palette(view, window, cx)).ok();
                    }
                    was_open = open;
                })
                .detach();
            root
        });
        if let Err(error) = opened {
            eprintln!("could not open the window: {error:#}");
            cx.quit();
            return;
        }
        if let Some(fixture) = &fixture {
            fixture.start_scripts();
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
