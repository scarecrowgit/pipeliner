//! The header bar alone in a narrow window: long crumbs give way, the actions stay inside.

use gpui::{
    Bounds, Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, WindowBounds, WindowOptions, div,
    prelude::*, px,
};
use hub::Route;

use crate::{Header, HeaderBar};

/// A window showing one header bar across its width.
struct Bar {
    long: bool,
}

impl Render for Bar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (middle, title) = if self.long {
            (
                "PAY-1427 · Refund webhook times out when the merchant's endpoint is slow",
                "Reproducer found the retry loop in the webhook dispatcher and wants a decision",
            )
        } else {
            ("Inbox", "Short")
        };
        let header = Header::new()
            .crumb_to("Inbox", Route::Inbox { item: None })
            .crumb_to(middle, Route::Chats)
            .crumb(title)
            .action(div().w(px(120.)).h(px(32.)).child("Send back"))
            .action(div().w(px(90.)).h(px(32.)).child("Approve"));
        div().size_full().flex().flex_col().child(HeaderBar::new(header))
    }
}

fn open(cx: &mut TestAppContext, width: f32, long: bool) -> &'static mut VisualTestContext {
    cx.update(|cx| {
        ui::motion::set_reduced(true);
        ui::keys::init(cx);
    });
    let window = cx
        .update(|cx| {
            let bounds = Bounds::new(gpui::point(px(0.), px(0.)), gpui::size(px(width), px(200.)));
            cx.open_window(
                WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
                move |_, cx| cx.new(|_| Bar { long }),
            )
        })
        .expect("a test window");
    let vcx = VisualTestContext::from_window(window.into(), cx).into_mut();
    vcx.run_until_parked();
    vcx
}

#[gpui::test]
fn a_long_trail_gives_way_and_the_actions_stay_in_the_bar(cx: &mut TestAppContext) {
    let cx = open(cx, 640., true);
    let header = cx.debug_bounds("header").expect("the bar");
    let actions = cx.debug_bounds("header-actions").expect("the actions");
    let title = cx.debug_bounds("header-title").expect("the title");
    let middle = cx.debug_bounds("header-crumb-1").expect("the middle crumb");
    assert!(actions.right() <= header.right(), "the actions stay in the bar");
    assert_eq!(actions.size.width, px(218.), "the actions keep their size");
    assert!(middle.right() <= actions.left(), "the middle crumb ends before the actions");
    assert!(title.right() <= actions.left(), "so does the title: {:?} vs {:?}", title.right(), actions.left());
    assert!(title.size.width > px(40.), "the title keeps some room");
}

#[gpui::test]
fn short_crumbs_keep_their_width(cx: &mut TestAppContext) {
    let cx = open(cx, 1200., false);
    let first = cx.debug_bounds("header-crumb-0").expect("the first crumb");
    let middle = cx.debug_bounds("header-crumb-1").expect("the middle crumb");
    // "Inbox" in the heading style is wider than this, and both are drawn whole.
    assert!(first.size.width > px(30.));
    assert_eq!(first.size.width, middle.size.width, "both say Inbox, both whole");
}
