//! The Cmd-K chat history palette: a centred overlay with a search field over the user's chats,
//! filter chips, grouped results with the match marked, and a preview of the selected chat.
//!
//! # Mounting
//!
//! The router decides when it is open: `Router::open_overlay(Overlay::Palette)` (the shell binds
//! Cmd-K to `toggle_overlay`). The shell then builds a new [`Palette`] and opens it as a dialog
//! layer of the overlay host (`Overlays::open_dialog_view`), which draws the scrim, centres the
//! panel and fades it in and out; when the overlay leaves the router the shell closes the layer:
//!
//! ```ignore
//! // once at startup, after ui::init and ui_rich::bind_keys
//! palette::init(cx);
//!
//! // in the shell, when the router opens `Overlay::Palette`
//! let palette = cx.new(|cx| Palette::new(&hub, window, cx));
//! ui::overlays(window, cx).open_dialog_view("palette", palette, close_the_router_overlay, window, cx);
//! ```
//!
//! A new palette starts from an empty query and focuses its field when first drawn. It closes
//! by clearing the overlay in the router (Escape, the Esc key cap), by navigating to the chosen
//! chat, which clears every overlay, or by a press on the layer's scrim.
//!
//! # Keys
//!
//! | Key | Does |
//! | --- | --- |
//! | Up, Down | move the selection, wrapping round at the ends |
//! | PageUp, PageDown | move a page, stopping at the first and last row |
//! | Enter | open the selected chat and close |
//! | Escape | close (it does not clear the query first: the mockup does not hint at that) |
//! | Cmd/Ctrl-P | pin or unpin the selected chat |
//! | Cmd-Backspace | archive the selected chat, only while the field is empty |
//!
//! The pointer moves the selection by hovering and opens a chat by clicking. Typing `in:<repo>`
//! followed by a space scopes the search to that repo; the word becomes a tag before the field and
//! a press on the tag removes it.

mod facets;
mod groups;
mod labels;
mod palette;
mod preview;
mod render;
mod row;
mod snippet;

#[cfg(test)]
mod tests;

use gpui::{App, KeyBinding, actions};

pub use palette::Palette;

actions!(
    palette,
    [
        /// Move the selection a page up.
        PageUp,
        /// Move the selection a page down.
        PageDown,
        /// Pin the selected chat, or unpin it.
        PinSelected,
    ]
);

/// The key context of the palette's root, for keymaps that want to add to it.
pub const CONTEXT: &str = "Palette";

/// Binds the palette's keys. Call once at startup, after `ui::init` and `ui_rich::bind_keys`.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("pageup", PageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", PageDown, Some(CONTEXT)),
        KeyBinding::new("cmd-p", PinSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-p", PinSelected, Some(CONTEXT)),
    ]);
}
