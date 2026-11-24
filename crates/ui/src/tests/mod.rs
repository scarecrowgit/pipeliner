//! Tests of the kit: the GPUI test harness itself, and clicks on every interactive component.

mod additions;
mod interactions;
mod keyboard;
mod overlays;

use gpui::{Context, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext, Window, div, prelude::*, px};

struct Counter {
    clicks: usize,
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("button")
                .debug_selector(|| "button".into())
                .w(px(120.))
                .h(px(40.))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.clicks += 1;
                    cx.notify();
                }))
                .child(format!("{} clicks", self.clicks)),
        )
    }
}

/// The UI test harness works in this workspace: a view renders in a test window, its
/// elements can be found by selector, and a simulated click reaches the listener.
#[gpui::test]
fn a_simulated_click_reaches_a_listener(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| Counter { clicks: 0 });
    let bounds = cx.debug_bounds("button").expect("the button was laid out");
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    view.read_with(cx, |counter, _| assert_eq!(counter.clicks, 1));
}
