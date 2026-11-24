//! The queue's keys: J/K and arrows move, 1–9 pick a variant, R sends back, Enter follows
//! the open item.

use gpui::{Context, KeyDownEvent, Window};

use crate::view::InboxView;

impl InboxView {
    pub(crate) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() || self.typing(window, cx) {
            return;
        }
        match keystroke.key.as_str() {
            "up" | "k" => self.move_selection(-1, window, cx),
            "down" | "j" => self.move_selection(1, window, cx),
            "enter" => self.activate(window, cx),
            "r" => self.open_send_back(window, cx),
            digit if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
                let n = u32::from(digit.as_bytes()[0] - b'0');
                if n > 0 {
                    self.pick_variant(n, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Enter: goes where the open item leads (a review is decided with its own buttons).
    fn activate(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.review.is_some() {
            return;
        }
        let Some(item) = self.current_item(cx) else {
            return;
        };
        if let Some(target) = crate::other::main_target(&item) {
            hub::open(target, cx);
        }
    }
}
