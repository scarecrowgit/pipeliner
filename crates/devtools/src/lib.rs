//! Component gallery, mockup-versus-window screenshots and the dev-driver client.
//!
//! The outside half of the dev driver (the inside half is `ui::dev`):
//!
//! - [`driver`]: a client for the app's `HARNESS_DEV_SOCKET` and [`launch`](driver::launch),
//!   which starts an app binary with a fresh socket and kills it on drop.
//! - [`niri`]: find the app's window by pid, resize it, screenshot it.
//! - [`mockup`]: render a `screens-v2` mockup to PNG with headless Firefox.
//! - [`compare`]: side-by-side and heat-map PNGs of a mockup and a window shot.
//! - [`script`]: the text format of `harness-dev run --script`.
//!
//! The `harness-dev` binary wraps all of it; run it inside the Nix shell so GPUI finds
//! wayland and vulkan:
//! `nix-shell shell.nix --run 'cargo run -p devtools --bin harness-dev -- <command>' </dev/null`.

pub mod compare;
pub mod driver;
pub mod mockup;
pub mod niri;
pub mod process;
pub mod script;
