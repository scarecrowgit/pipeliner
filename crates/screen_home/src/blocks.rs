//! What every block of Home shares: the load state it draws from, skeletons, the error state
//! with its retry, and the probe names.

use std::sync::Arc;

use gpui::{AnyElement, App, InteractiveElement, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::{LoadState, Loadable};
use ui::dev::{Probe, Probed};
use ui::motion::{self, Look};
use ui::prelude::*;

/// Where a block's data stands, owned so a view can draw while it borrows its context.
pub(crate) enum Block<T> {
    /// Asked for, not there yet.
    Loading,
    /// The last load failed; the message is the engine's.
    Failed(SharedString),
    /// The data.
    Ready(Arc<T>),
}

/// The state of a block from the model's slot (`None`: not asked for yet).
pub(crate) fn block<T>(slot: Option<&Loadable<T>>) -> Block<T> {
    let Some(slot) = slot else { return Block::Loading };
    match slot.state() {
        LoadState::Failed(message) => Block::Failed(message.clone()),
        _ => slot.value().map_or(Block::Loading, |value| Block::Ready(value.clone())),
    }
}

/// Names an element for the dev driver and for tests, the same way.
pub(crate) fn probed<E>(el: E, name: impl Into<String>) -> Probed<E>
where
    E: InteractiveElement + IntoElement,
{
    let name: String = name.into();
    let selector = name.clone();
    el.debug_selector(move || selector).probe(name)
}

/// A grey bar standing where a value will be.
pub(crate) fn skeleton(width: f32, height: f32) -> gpui::Div {
    div().flex_none().w(px(width)).h(px(height)).rounded(px(RADIUS_SM)).bg(SURFACE_HOVER)
}

/// What failed, why, and a retry that asks hub again.
pub(crate) fn error_view(
    what: &str,
    message: SharedString,
    name: &'static str,
    retry: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    col()
        .flex_none()
        .w_full()
        .gap(px(SPACE_2))
        .items_start()
        .p(px(SPACE_4))
        .child(div().type_style(BODY_STRONG).child(format!("Could not load {what}")))
        .child(
            div()
                .w_full()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(truncate_block(message, LABEL.line)),
        )
        .child(probed(
            div().child(Button::new(SharedString::from(format!("home-retry-button-{name}")), "Retry").small().icon(IconName::Replay).on_click(
                move |_, window, cx| retry(window, cx),
            )),
            format!("home-retry-{name}"),
        ))
        .into_any_element()
}

/// A short sentence in a block that has nothing to show.
pub(crate) fn empty_view(name: &'static str, title: &str, hint: &str) -> AnyElement {
    probed(col(), name)
        .flex_none()
        .w_full()
        .items_center()
        .gap(px(SPACE_1))
        .py(px(SPACE_6))
        .child(div().type_style(BODY_STRONG).child(title.to_string()))
        .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(hint.to_string()))
        .into_any_element()
}

/// The muted "text →" link at the foot of a card; the caller adds the click.
pub(crate) fn footer_link(
    id: &'static str,
    text: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Stateful<gpui::Div> {
    let faded = motion::hover_fade_state(
        id,
        Look::bg(TRANSPARENT).fg(TEXT_MUTED),
        Look::bg(TRANSPARENT).fg(TEXT),
        window,
        cx,
    );
    let color = faded.fg.unwrap_or(TEXT_MUTED);
    faded
        .apply(div().id(id))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(SPACE_1_5))
        .cursor_pointer()
        .child(text.into())
        .child(icon(IconName::ArrowRight, 12., color))
}
