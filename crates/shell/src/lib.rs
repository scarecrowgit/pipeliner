//! App shell: sidebar, rail, content frame, header, overlay host, project and spend popovers.
//!
//! The shell is the chrome every screen lives in. It knows no screen crate: the `app` binary
//! registers one [`Screen`] per [`ScreenKind`] in a [`Screens`] registry and hands it to
//! [`mount`], which returns the window's root (the overlay host around the [`Shell`]).
//!
//! ```ignore
//! shell::init(cx);                                    // key bindings, once
//! let hub = hub::Hub::new(api, cx);
//! let screens = Screens::new().add(ScreenKind::Home, Keep::One, |_, window, cx| {
//!     let home = cx.new(|cx| HomeScreen::new(window, cx));
//!     let h = home.clone();
//!     Mounted::new(home).header(move |_, cx| h.read(cx).header(cx))
//! });
//! cx.open_window(options, |window, cx| {
//!     let (shell, root) = shell::mount(screens, window, cx);
//!     let hub = hub::hub(cx);
//!     shell.update(cx, |shell, cx| {
//!         shell.set_palette(move |window, cx| cx.new(|cx| Palette::new(&hub, window, cx)).into(), window, cx)
//!     });
//!     root
//! })?;
//! ```
//!
//! - **Caching**: see [`Keep`]. A view is built the first time its key is shown and reused
//!   on every return; `Keep::One` shares one view across a kind's routes, `Keep::PerRoute(n)`
//!   keeps one per route for the `n` most recently shown.
//! - **Header**: each [`Mounted`] screen fills a [`Header`] every frame (breadcrumb, badges,
//!   actions); without one the header shows the section's label.
//! - **Keys** ([`init`]): ⌘K / Ctrl-K opens the palette (the router's [`hub::Overlay::Palette`],
//!   built afresh by the factory [`Shell::set_palette`] takes), ⌘N / Ctrl-N goes to the new-chat page,
//!   ⌘\ / Ctrl-\ collapses or expands the sidebar (saved in the settings), ⌘O opens the
//!   project switcher, ⌘, the current project's settings, ⌘1–⌘5 switch projects.
//! - **Notices**: every [`hub::HubNotice`] becomes a toast through the overlay host.

mod cache;
mod collapse;
mod header;
mod notice;
mod placeholder;
mod screen;
mod shell;
mod sidebar;

#[cfg(test)]
mod tests;

use gpui::{App, AppContext as _, Entity, KeyBinding, Window, actions};
use ui::OverlayHost;

pub use collapse::{is_collapsed, set_collapsed, toggle as toggle_sidebar};
pub use header::HeaderBar;
pub use notice::toast_for;
pub use placeholder::Placeholder;
pub use screen::{Crumb, Header, HeaderFn, Keep, Mounted, Screen, ScreenKind, Screens, ShowFn, section_label};
pub use shell::{PALETTE_LAYER, Shell};
pub use sidebar::{Shown, Sidebar};

actions!(
    shell,
    [
        /// Open the chat search and command palette (⌘K), or close it when it is open.
        TogglePalette,
        /// Go to the new-chat page (⌘N).
        NewChat,
        /// Collapse the sidebar to the rail, or expand it (⌘\).
        ToggleSidebar,
        /// Open the project switcher (⌘O).
        OpenProjects,
        /// Open the current project's settings (⌘,).
        ProjectSettings,
        /// Switch to the first project (⌘1).
        SwitchProject1,
        /// Switch to the second project (⌘2).
        SwitchProject2,
        /// Switch to the third project (⌘3).
        SwitchProject3,
        /// Switch to the fourth project (⌘4).
        SwitchProject4,
        /// Switch to the fifth project (⌘5).
        SwitchProject5,
    ]
);

/// The shell's key bindings, each with Cmd and with Ctrl.
pub fn bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for modifier in ["cmd", "ctrl"] {
        bindings.push(KeyBinding::new(&format!("{modifier}-k"), TogglePalette, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-n"), NewChat, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-\\"), ToggleSidebar, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-o"), OpenProjects, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-,"), ProjectSettings, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-1"), SwitchProject1, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-2"), SwitchProject2, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-3"), SwitchProject3, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-4"), SwitchProject4, None));
        bindings.push(KeyBinding::new(&format!("{modifier}-5"), SwitchProject5, None));
    }
    bindings
}

/// Bind the shell's keys and their app-wide handlers. Call once at start, after `ui::init`.
/// The handlers act through the hub, so they work whatever has focus.
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
    cx.on_action(|_: &TogglePalette, cx| {
        hub::router(cx).update(cx, |router, cx| router.toggle_overlay(hub::Overlay::Palette, cx));
    });
    cx.on_action(|_: &NewChat, cx| hub::navigate(hub::Route::NewChat, cx));
    cx.on_action(|_: &ToggleSidebar, cx| collapse::toggle(cx));
    cx.on_action(|_: &OpenProjects, cx| in_active_window(cx, sidebar::projects::open));
    cx.on_action(|_: &ProjectSettings, cx| in_active_window(cx, sidebar::projects::form::open_edit));
    cx.on_action(|_: &SwitchProject1, cx| sidebar::projects::switch_to_nth(1, cx));
    cx.on_action(|_: &SwitchProject2, cx| sidebar::projects::switch_to_nth(2, cx));
    cx.on_action(|_: &SwitchProject3, cx| sidebar::projects::switch_to_nth(3, cx));
    cx.on_action(|_: &SwitchProject4, cx| sidebar::projects::switch_to_nth(4, cx));
    cx.on_action(|_: &SwitchProject5, cx| sidebar::projects::switch_to_nth(5, cx));
}

/// Run `f` in the active window, for shortcuts that open a popover or a dialog.
fn in_active_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App)) {
    if let Some(window) = cx.active_window() {
        window.update(cx, |_, window, cx| f(window, cx)).ok();
    }
}

/// Build the shell for `window` and the overlay host around it: return the host as the
/// window's root view. The overlay host is mounted once here, for the whole window.
pub fn mount(screens: Screens, window: &mut Window, cx: &mut App) -> (Entity<Shell>, Entity<OverlayHost>) {
    let shell = cx.new(|cx| Shell::new(screens, window, cx));
    let root = OverlayHost::wrap(shell.clone(), window, cx);
    (shell, root)
}
