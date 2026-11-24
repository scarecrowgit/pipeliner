//! The header bar of a screen (`.hd`): breadcrumb at the left, badges right after it, actions
//! at the right, 56px tall over a hairline.

use gpui::{App, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use crate::screen::{Crumb, Header};

/// Draws a [`Header`]. Probes: `header`, `header-crumb-<n>` for each link.
#[derive(IntoElement)]
pub struct HeaderBar {
    header: Header,
}

impl HeaderBar {
    /// The bar for `header`.
    pub fn new(header: Header) -> Self {
        Self { header }
    }
}

impl RenderOnce for HeaderBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Header { crumbs, after, actions, .. } = self.header;
        let last = crumbs.len().saturating_sub(1);
        let mut trail = row().gap(px(SPACE_1_5)).min_w_0().flex_shrink().type_style(HEADING);
        for (ix, crumb) in crumbs.into_iter().enumerate() {
            if ix < last {
                trail = trail
                    .child(crumb_link(ix, crumb, window, cx))
                    .child(icon(IconName::ChevronRight, 14., TEXT_DISABLED));
            } else {
                trail = trail.child(truncate(crumb.label).text_color(TEXT));
            }
        }
        row()
            .id("header")
            .debug_selector(|| "header".into())
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .gap(px(SPACE_2))
            .pl(px(SPACE_5))
            .pr(px(SPACE_2))
            .border_b_1()
            .border_color(LINE)
            .child(trail)
            .child(after_row(after))
            .child(grow())
            .child(row().flex_none().gap(px(SPACE_2)).children(actions))
            .probe("header")
    }
}

/// The badges and chips after the title. They keep their size while there is room; in a
/// window too narrow for the whole bar they are clipped from the right before the title is
/// cut, and the actions always stay on screen.
fn after_row(after: Vec<gpui::AnyElement>) -> impl IntoElement {
    let mut clip = row().gap(px(SPACE_2)).min_w_0().overflow_hidden();
    // Gives way far sooner than the title, whose shrink factor is 1.
    clip.style().flex_shrink = Some(1000.);
    clip.children(after.into_iter().map(|el| div().flex_none().child(el)))
}

/// A crumb before the last one: muted, regular weight, `text` on hover.
fn crumb_link(ix: usize, crumb: Crumb, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let name: SharedString = format!("header-crumb-{ix}").into();
    let selector = name.to_string();
    let faded = motion::hover_fade_state(
        name.clone(),
        Look::bg(TRANSPARENT).fg(TEXT_MUTED),
        Look::bg(TRANSPARENT).fg(TEXT),
        window,
        cx,
    );
    let el = div()
        .id(gpui::ElementId::Name(name.clone()))
        .debug_selector(move || selector)
        .flex_none()
        .whitespace_nowrap()
        .font_weight(gpui::FontWeight::NORMAL)
        .when(crumb.route.is_some(), |d| d.cursor_pointer());
    let route = crumb.route;
    faded
        .apply(el)
        .child(crumb.label)
        .when_some(route, |d, route| d.on_click(move |_, _, cx| hub::navigate(route.clone(), cx)))
        .probe(name)
}
