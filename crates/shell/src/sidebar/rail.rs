//! The collapsed sidebar (`.rail`): 56px of icon buttons with counts and pips, the live
//! pipelines, and today's spend at the foot.

use gpui::{AnyElement, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::{Route, Section};
use model::PipelineId;
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use super::foot::{self, spend_bar, spend_texts};
use super::{NAV, Sidebar, logo, open_palette};
use crate::collapse;
use crate::screen::section_label;

/// The rail's spend block, an anchor for the spend popover.
const SPEND_ANCHOR: &str = "rail-spend";

impl Sidebar {
    pub(super) fn render_rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active(cx);
        let inbox_waiting = self.inbox.read(cx).waiting();
        let chats_waiting = self.chats.read(cx).waiting();
        self.shown.inbox_waiting = inbox_waiting;
        self.shown.chats_waiting = chats_waiting;
        self.shown.chats.clear();
        let project = self.project_name(cx);

        let expand_faded = motion::hover_fade_state(
            "sidebar-expand",
            Look::bg(TRANSPARENT),
            Look::bg(SURFACE_RAISED),
            window,
            cx,
        );
        let expand = div()
            .id("sidebar-expand")
            .debug_selector(|| "sidebar-expand".into())
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(RAIL_BUTTON))
            .rounded(px(RADIUS_MD))
            .cursor_pointer()
            .tooltip(Tooltip::with_kbd(format!("Expand sidebar · project {project}"), "⌘\\"))
            .on_click(|_, _, cx| collapse::toggle(cx));
        let expand = expand_faded.apply(expand).child(logo(24.)).probe("sidebar-expand");

        let nav = NAV.iter().map(|entry| {
            let route = entry.route;
            let label = section_label(entry.section);
            let mut button = IconButton::new(entry.probe, entry.icon)
                .rail()
                .current(active == entry.section)
                .on_click(move |_, _, cx| hub::navigate(route(), cx));
            let tooltip = match entry.section {
                Section::Inbox if inbox_waiting > 0 => {
                    button = button.count(inbox_waiting.to_string());
                    format!("{label} · {inbox_waiting} waiting")
                }
                Section::Chats if chats_waiting > 0 => {
                    button = button.pip(WARNING);
                    format!("{label} · {chats_waiting} waiting")
                }
                _ => label.to_string(),
            };
            button.tooltip(tooltip).probe(entry.probe)
        });

        let live: Vec<_> = self.pipelines(cx).into_iter().filter(|row| row.status == Status::Live).collect();
        let pipelines = live.into_iter().map(|row| {
            let probe: SharedString = format!("sidebar-pipeline-{}", row.reference.id()).into();
            let pipeline = PipelineId::new(row.reference.id());
            IconButton::new(ElementId::Name(probe.clone()), IconName::Pipelines)
                .rail()
                .pip(SUCCESS)
                .tooltip(format!("{} · live", row.name))
                .on_click(move |_, _, cx| hub::navigate(Route::Editor { pipeline: pipeline.clone(), node: None }, cx))
                .probe(probe)
        });

        let spend = self.ops.read(cx).spend_today().value().map(|s| (**s).clone()).unwrap_or_default();
        let (cost, of) = spend_texts(&spend);
        let ops = self.ops.clone();
        let spend_block = col()
            .id(SPEND_ANCHOR)
            .debug_selector(|| SPEND_ANCHOR.into())
            .mt_auto()
            .flex_none()
            .items_center()
            .gap(px(SPACE_1_5))
            .cursor_pointer()
            .tooltip(Tooltip::text(format!("Spend today: {cost} {of}")))
            .on_click(move |_, window, cx| foot::open_popover(ops.clone(), SPEND_ANCHOR, Placement::above(), window, cx))
            .child(div().type_style(MONO_SM).text_color(TEXT_MUTED).tabular().child(cost.clone()))
            .children(spend_bar(&spend).map(|bar| bar.width(SPACE_8)))
            .popover_anchor(SPEND_ANCHOR)
            .probe(SPEND_ANCHOR);

        col()
            .id("rail")
            .debug_selector(|| "rail".into())
            .w(px(SIDEBAR_RAIL_WIDTH))
            .flex_none()
            .h_full()
            .items_center()
            .gap(px(SPACE_1))
            .py(px(SPACE_3))
            .child(expand)
            .child(
                IconButton::new("sidebar-search", IconName::Search)
                    .rail()
                    .tooltip("Search · ⌘K")
                    .on_click(|_, _, cx| open_palette(cx))
                    .probe("sidebar-search"),
            )
            .child(
                IconButton::new("sidebar-new-chat", IconName::Edit)
                    .rail()
                    .tooltip("New chat · ⌘N")
                    .on_click(|_, _, cx| hub::navigate(Route::NewChat, cx))
                    .probe("sidebar-new-chat"),
            )
            .child(rule())
            .child(col().flex_none().items_center().gap(px(SPACE_1)).children(nav))
            .child(rule())
            .children(pipelines)
            .child(spend_block)
            .into_any_element()
    }
}

/// `.rail hr`: a 24px hairline between groups.
fn rule() -> impl IntoElement {
    div().flex_none().w(px(SPACE_6)).h(px(1.)).my(px(SPACE_1_5)).bg(LINE)
}
