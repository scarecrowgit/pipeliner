//! The shell over the fixture backend, with stand-in screens that show their route as text.
//!
//! `cargo run -p shell --example preview -- --fixture <name>`, where `<name>` is one of
//! `hub::fixtures::SCENARIOS` (default `home`). The dev driver listens when
//! `HARNESS_DEV_SOCKET` is set; probes are listed in the `shell` crate docs.

use std::sync::Arc;

use gpui::{
    App, Application, Bounds, Context, IntoElement, Render, SharedString, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};
use hub::fixtures::{FixtureApi, SCENARIOS, start_route};
use hub::{Hub, Overlay, Route};
use shell::{Header, Keep, Mounted, ScreenKind, Screens, section_label};
use ui::dev::Probe;
use ui::prelude::*;

/// A stand-in screen: the route it was shown for, in mono.
struct RouteText {
    route: Route,
}

impl Render for RouteText {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(SPACE_6))
            .flex()
            .flex_col()
            .gap(px(SPACE_2))
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Stand-in screen for"))
            .child(div().type_style(MONO).text_color(TEXT).child(format!("{:?}", self.route)))
            .probe_value("preview-route", format!("{:?}", self.route))
    }
}

/// A stand-in palette in the shell's palette slot.
struct PaletteStandIn;

impl Render for PaletteStandIn {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        col()
            .id("palette-stand-in")
            .occlude()
            .mt(px(120.))
            .w(px(DIALOG_WIDTH))
            .p(px(SPACE_5))
            .gap(px(SPACE_3))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_DIALOG))
            .bg(SURFACE)
            .child(div().type_style(TITLE_SM).child("Search chats"))
            .child(div().text_color(TEXT_MUTED).child("The palette crate mounts here."))
            .child(
                row().child(grow()).child(
                    Button::new("palette-close", "Close")
                        .on_click(|_, _, cx| {
                            hub::router(cx).update(cx, |router, cx| router.close_overlay(Overlay::Palette, cx))
                        })
                        .probe("palette-close"),
                ),
            )
    }
}

/// Every kind gets a stand-in; chat sessions are kept per session, as the app keeps them.
fn screens() -> Screens {
    ScreenKind::ALL.into_iter().fold(Screens::new(), |screens, kind| {
        let keep = if kind == ScreenKind::Chat { Keep::PerRoute(8) } else { Keep::One };
        screens.add(kind, keep, move |route, _, cx| {
            let view = cx.new(|_| RouteText { route: route.clone() });
            let shown = view.clone();
            let mounted = Mounted::new(view.clone()).on_show(move |route, _, cx| {
                shown.update(cx, |view, cx| {
                    view.route = route.clone();
                    cx.notify();
                })
            });
            mounted.header(move |_, cx| header(&view.read(cx).route))
        })
    })
}

/// A breadcrumb like the mockups': the section as a link, then what the route names.
fn header(route: &Route) -> Header {
    let section = section_label(route.section());
    let detail: Option<SharedString> = match route {
        Route::Chat { session } => Some(session.to_string().into()),
        Route::NewChat => Some("New session".into()),
        Route::Issues { issue: Some(key) } => Some(key.to_string().into()),
        Route::Agents { runnable: Some(runnable) } => Some(runnable.to_string().into()),
        Route::Editor { pipeline, .. } => Some(pipeline.to_string().into()),
        _ => None,
    };
    let back = match route {
        Route::Chat { .. } | Route::NewChat => Route::Chats,
        Route::Issues { .. } => Route::Issues { issue: None },
        Route::Agents { .. } | Route::Editor { .. } => Route::Agents { runnable: None },
        other => other.clone(),
    };
    match detail {
        Some(detail) => Header::new()
            .crumb_to(section, back)
            .crumb(detail)
            .action(Button::new("preview-action", "Stand-in action").ghost().small()),
        None => Header::title(section).action(Button::new("preview-action", "Stand-in action").ghost().small()),
    }
}

fn fixture_name() -> String {
    let mut args = std::env::args().skip(1);
    let mut name = "home".to_string();
    while let Some(arg) = args.next() {
        if arg == "--fixture" {
            if let Some(value) = args.next() {
                name = value;
            }
        } else if let Some(value) = arg.strip_prefix("--fixture=") {
            name = value.to_string();
        }
    }
    name
}

fn main() {
    let name = fixture_name();
    let Some(api) = FixtureApi::scenario(&name) else {
        eprintln!("unknown fixture `{name}`; one of: {}", SCENARIOS.join(", "));
        std::process::exit(2);
    };
    let api = Arc::new(api);
    Application::new().with_assets(ui::assets::Assets).run(move |cx: &mut App| {
        ui::init(cx);
        ui_rich::bind_keys(cx);
        shell::init(cx);
        Hub::new(api.clone(), cx);
        if let Some((route, overlay)) = start_route(&name) {
            hub::navigate(route, cx);
            if let Some(overlay) = overlay {
                hub::router(cx).update(cx, |router, cx| router.open_overlay(overlay, cx));
            }
        }
        let bounds = Bounds::centered(None, size(px(1440.), px(960.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions { title: Some(format!("Shell · {name}").into()), ..Default::default() }),
                app_id: Some("harness-demo".into()),
                window_min_size: Some(size(px(900.), px(600.))),
                ..Default::default()
            },
            |window, cx| {
                let (shell, root) = shell::mount(screens(), window, cx);
                shell.update(cx, |shell, cx| shell.set_palette(|_, cx| cx.new(|_| PaletteStandIn).into(), window, cx));
                root
            },
        );
        if let Err(error) = opened {
            eprintln!("could not open the window: {error:#}");
            cx.quit();
            return;
        }
        api.start_scripts();
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
