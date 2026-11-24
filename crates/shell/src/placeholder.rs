//! What the content frame shows for a screen nobody registered.

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px};
use ui::dev::Probe;
use ui::prelude::*;

use crate::screen::{Header, Mounted, ScreenKind, section_label};

/// A neutral empty screen: one muted line in the middle of the frame. Probe:
/// `screen-placeholder`.
pub struct Placeholder {
    kind: ScreenKind,
}

impl Placeholder {
    /// The placeholder of `kind`.
    pub fn new(kind: ScreenKind) -> Self {
        Self { kind }
    }

    /// The kind it stands in for.
    pub fn kind(&self) -> ScreenKind {
        self.kind
    }
}

impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("screen-placeholder")
            .debug_selector(|| "screen-placeholder".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(SPACE_1))
            .child(div().type_style(BODY_STRONG).text_color(TEXT_SECONDARY).child("Nothing here yet"))
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("This screen is not part of this build."))
            .probe("screen-placeholder")
    }
}

/// The mounted placeholder of `kind`: its section's label as the title.
pub(crate) fn mounted(kind: ScreenKind, route: &hub::Route, cx: &mut gpui::App) -> Mounted {
    let title = section_label(route.section());
    Mounted::new(cx.new(|_| Placeholder::new(kind))).header(move |_, _| Header::title(title))
}
