//! End to end against a real window (opens one, so it is ignored by default). Build the demo
//! first, then run inside the Nix shell:
//!
//! ```text
//! cargo build -p devtools --example driver_demo
//! HARNESS_DEV_OUT=/some/dir cargo test -p devtools --test driver_window -- --ignored
//! ```
//!
//! Real mouse and key events: clicks a kit `Button`, hovers a card, opens a menu and closes
//! it by clicking away, drags a knob, wheel-scrolls a list, activates a focused control with
//! Enter and Space, and types into a field. Under niri it screenshots into
//! `$HARNESS_DEV_OUT` (`demo-hover.png`, `demo-menu.png`, `driver-demo.png`).

use std::path::PathBuf;
use std::time::Duration;

use devtools::driver::{Running, at, launch, probe};
use devtools::niri;

fn demo_binary() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    target.join("debug/examples/driver_demo")
}

/// Screenshot into `$HARNESS_DEV_OUT/<name>` when niri knows the window.
fn shot(app: &mut Running, window: Option<u64>, name: &str) {
    let Some(id) = window else { return };
    let out = std::env::var_os("HARNESS_DEV_OUT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    app.driver.wait().expect("frame before the shot");
    let png = out.join(name);
    niri::screenshot(id, &png).expect("screenshot");
    assert!(image::open(&png).expect("png").width() > 100);
}

#[test]
#[ignore = "opens a window; needs a Wayland session and the built driver_demo example"]
fn drives_the_demo_window() {
    let binary = demo_binary();
    assert!(binary.exists(), "build it first: cargo build -p devtools --example driver_demo");
    let mut app = launch(&binary, &[], &[]).expect("launch the demo");
    let window = niri::window_for_pid(app.pid, Duration::from_secs(5)).ok().map(|w| w.id);
    let d = &mut app.driver;

    d.wait().expect("first frame");
    let probes = d.probes().expect("probes");
    let names: Vec<&str> = probes.iter().map(|p| p.name.as_str()).collect();
    for want in ["increment", "count", "hover-card", "open-menu", "track", "knob", "control", "field", "list"] {
        assert!(names.contains(&want), "missing probe {want} in {names:?}");
    }
    assert!(!names.contains(&"menu"), "the menu starts closed");

    // A kit Button, found by a probe around it, clicked through GPUI's hit testing.
    assert_eq!(d.text("count").expect("count"), "0");
    d.click(probe("increment")).expect("click");
    d.dblclick(probe("increment")).expect("double click");
    assert_eq!(d.text("count").expect("count"), "3");

    // Click-away: open the menu, then click empty space.
    d.click(probe("open-menu")).expect("open the menu");
    assert_eq!(d.text("menu-state").expect("menu"), "open");
    assert!(d.probes().expect("probes").iter().any(|p| p.name == "menu"));
    shot(&mut app, window, "demo-menu.png");
    let d = &mut app.driver;
    d.click(at(700., 300.)).expect("click away");
    assert_eq!(d.text("menu-state").expect("menu"), "closed");
    d.wait().expect("frame");
    assert!(!d.probes().expect("probes").iter().any(|p| p.name == "menu"), "menu probe gone");

    // Drag the knob to the right end of its track.
    let track = d.probes().expect("probes").into_iter().find(|p| p.name == "track").expect("track");
    d.drag(probe("knob"), at(track.x + track.width - 4., track.y + track.height / 2.)).expect("drag");
    let knob: f32 = d.text("knob-x").expect("knob").parse().expect("number");
    assert!(knob > 300., "knob moved to {knob}");

    // Wheel over a scroll container.
    d.scroll(probe("list"), 0., -60.).expect("scroll");
    d.wait().expect("frame");
    let offset: f32 = d.text("list").expect("offset").parse().expect("number");
    assert!(offset >= 59., "list scrolled to {offset}");

    // Enter and Space activate a focused control (keyboard click needs key-up).
    d.click(probe("control")).expect("click focuses the control");
    d.wait().expect("frame with focus");
    assert_eq!(d.text("activations").expect("activations"), "1");
    d.key(&["enter"]).expect("enter");
    d.wait().expect("frame");
    d.key(&["space"]).expect("space");
    d.wait().expect("frame");
    assert_eq!(d.text("activations").expect("activations"), "3");

    // Typing reaches the focused input.
    d.click(probe("field")).expect("focus the field");
    d.wait().expect("frame with the input handler");
    assert_eq!(d.type_text("Hello world").expect("type"), 0);
    d.wait().expect("frame after typing");
    assert_eq!(d.text("echo").expect("echo"), "Hello world");

    // Hover last, so the screenshot shows the hover style.
    d.hover(probe("hover-card")).expect("hover");
    shot(&mut app, window, "demo-hover.png");
    shot(&mut app, window, "driver-demo.png");
    app.stop();
}
