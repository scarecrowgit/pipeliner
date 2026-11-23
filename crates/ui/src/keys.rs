//! The kit's key bindings: one place that names the keyboard actions every view shares and the
//! key contexts the kit's components declare.
//!
//! Screens handle these actions instead of binding keys of their own: a view that wants to
//! react to Escape adds `.on_action(|_: &keys::Cancel, window, cx| ..)` on its root. Escape
//! reaches the view only when no overlay is open (the overlay host closes the top layer
//! first). Primary-modifier shortcuts are bound twice, `cmd-` and `ctrl-`, so they work on
//! every platform.
//!
//! | Action | Keys | Context |
//! | --- | --- | --- |
//! | [`Cancel`] | Escape | everywhere |
//! | [`FocusNext`], [`FocusPrev`] | Tab, Shift-Tab | everywhere |
//! | [`Undo`] | Cmd/Ctrl-Z | everywhere (the toast's Undo when nothing else takes it) |
//! | [`Quit`] | Cmd/Ctrl-Q | everywhere |
//! | [`Confirm`] | Cmd/Ctrl-Enter | [`DIALOG`] |
//! | [`Confirm`], [`MoveUp`], [`MoveDown`], [`MoveFirst`], [`MoveLast`] | Enter, arrows, Home, End | [`MENU`] |
//! | [`MoveLeft`], [`MoveRight`], [`MoveUp`], [`MoveDown`], [`MoveFirst`], [`MoveLast`] | arrows, Home, End | [`CHOICE`] |
//! | [`MoveUp`], [`MoveDown`] | Up, Down | [`STEPPER`] |

use gpui::{App, KeyBinding, actions};

actions!(
    ui,
    [
        /// Close the top overlay, or cancel what the focused view is doing (Escape).
        Cancel,
        /// Run the primary action: Cmd/Ctrl-Enter in a dialog, Enter in a menu.
        Confirm,
        /// Move the selection up one row, or step a value up.
        MoveUp,
        /// Move the selection down one row, or step a value down.
        MoveDown,
        /// Move to the previous option of a horizontal group.
        MoveLeft,
        /// Move to the next option of a horizontal group.
        MoveRight,
        /// Move to the first row or option (Home).
        MoveFirst,
        /// Move to the last row or option (End).
        MoveLast,
        /// Move keyboard focus to the next control (Tab).
        FocusNext,
        /// Move keyboard focus to the previous control (Shift-Tab).
        FocusPrev,
        /// Undo the last destructive action (the toast's Undo).
        Undo,
        /// Quit the app.
        Quit,
    ]
);

/// Key context of an open popover or dialog layer (the overlay host sets it).
pub const OVERLAY: &str = "Overlay";
/// Key context of a [`Dialog`](crate::dialog::Dialog) panel.
pub const DIALOG: &str = "Dialog";
/// Key context of a [`Menu`](crate::menu::Menu).
pub const MENU: &str = "Menu";
/// Key context of a roving group: `Tabs`, `Segmented`, radio and option groups.
pub const CHOICE: &str = "Choice";
/// Key context of a focused `Stepper`.
pub const STEPPER: &str = "Stepper";

/// The same shortcut with `cmd-` and with `ctrl-`: `primary("enter")` is Cmd-Enter and
/// Ctrl-Enter.
fn primary<A: gpui::Action + Clone>(key: &str, action: A, context: Option<&str>) -> [KeyBinding; 2] {
    [
        KeyBinding::new(&format!("cmd-{key}"), action.clone(), context),
        KeyBinding::new(&format!("ctrl-{key}"), action, context),
    ]
}

/// Every binding of the kit, for [`init`] and for tests that want to inspect them.
pub fn bindings() -> Vec<KeyBinding> {
    let mut b = vec![
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrev, None),
        KeyBinding::new("up", MoveUp, Some(MENU)),
        KeyBinding::new("down", MoveDown, Some(MENU)),
        KeyBinding::new("home", MoveFirst, Some(MENU)),
        KeyBinding::new("end", MoveLast, Some(MENU)),
        KeyBinding::new("enter", Confirm, Some(MENU)),
        KeyBinding::new("left", MoveLeft, Some(CHOICE)),
        KeyBinding::new("right", MoveRight, Some(CHOICE)),
        KeyBinding::new("up", MoveUp, Some(CHOICE)),
        KeyBinding::new("down", MoveDown, Some(CHOICE)),
        KeyBinding::new("home", MoveFirst, Some(CHOICE)),
        KeyBinding::new("end", MoveLast, Some(CHOICE)),
        KeyBinding::new("up", MoveUp, Some(STEPPER)),
        KeyBinding::new("down", MoveDown, Some(STEPPER)),
    ];
    b.extend(primary("enter", Confirm, Some(DIALOG)));
    b.extend(primary("z", Undo, None));
    b.extend(primary("q", Quit, None));
    b
}

/// Bind the kit's keys and its app-wide fallbacks (Tab moves focus even in a window without
/// an overlay host; Quit quits). [`crate::init`] calls it.
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &FocusNext, cx| move_focus(true, cx));
    cx.on_action(|_: &FocusPrev, cx| move_focus(false, cx));
}

fn move_focus(next: bool, cx: &mut App) {
    let Some(window) = cx.active_window() else { return };
    window
        .update(cx, |_, window, cx| {
            crate::focus::set_focus_visible(true, window, cx);
            if next { window.focus_next() } else { window.focus_prev() }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_shortcuts_are_bound_with_cmd_and_ctrl() {
        let b = bindings();
        let bound = |typed: &str| {
            let k = gpui::Keystroke::parse(typed).expect("a keystroke");
            b.iter().any(|binding| binding.match_keystrokes(std::slice::from_ref(&k)) == Some(false))
        };
        for key in ["cmd-enter", "ctrl-enter", "cmd-z", "ctrl-z", "cmd-q", "ctrl-q", "escape", "tab", "shift-tab"] {
            assert!(bound(key), "{key} is not bound");
        }
    }
}
