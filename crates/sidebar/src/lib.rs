//! App sidebar: the full 232px navigation column, or the 56px icon rail on the
//! pipeline editor. Port of `reference/app_sidebar.html`.
//!
//! The sidebar reads the current screen from [`ui::nav`] on every render and
//! derives the mockup's `active` / `compact` / `current` props from it.

mod data;
mod tooltip;

use gpui::{
    ClickEvent, Context, ElementId, FontWeight, IntoElement, Render, SharedString, Stateful,
    Window, div, prelude::*, px,
};
use ui::nav::{self, Screen};
use ui::theme::*;
use ui::{Badge, Tone, icon, kbd, progress, status_dot};

use data::{NAV, RAIL_RUNNERS, RUNNERS, Runner};
use tooltip::tooltip;

/// Hover group shared by every clickable row, so icons brighten with their row.
const ROW_GROUP: &str = "sidebar-row";

pub struct Sidebar {}

/// What the screens pass to `<dc-import name="Sidebar" ...>` in the mockup.
struct Props {
    active: Option<Screen>,
    compact: bool,
    current: Option<&'static str>,
}

impl Sidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        if let Some(nav) = nav::nav_entity(cx) {
            cx.observe(&nav, |_, _, cx| cx.notify()).detach();
        }
        Self {}
    }

    fn props(cx: &gpui::App) -> Props {
        let screen = nav::current(cx);
        match screen {
            Screen::PipelineEditor => Props {
                active: None,
                compact: true,
                current: Some("pay-triage"),
            },
            Screen::Inbox | Screen::Issues => Props {
                active: Some(screen),
                compact: false,
                current: None,
            },
            Screen::Chat => Props {
                active: Some(screen),
                compact: false,
                current: Some("build"),
            },
            Screen::Agents => Props {
                active: Some(screen),
                compact: false,
                current: Some(selected_runner_id(cx)),
            },
        }
    }

    /// PAY triage opens the editor; every other entity is selected on the Agents screen.
    fn runner_click(
        runner: &'static Runner,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static {
        move |_, _, cx| match runner.target() {
            Screen::Agents => nav::select_agent(runner.name, true, cx),
            target => nav::navigate(target, cx),
        }
    }

    // ── Full sidebar ────────────────────────────────────────────────────

    fn render_full(&self, props: &Props, cx: &mut Context<Self>) -> gpui::Div {
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(SPACE_3))
            .child(workspace_row());

        for entry in NAV {
            let active = props.active == Some(entry.screen);
            let screen = entry.screen;
            let item = nav_item(("sidebar-nav", entry.screen as usize), active)
                .child(row_icon(
                    entry.icon,
                    16.,
                    if active { TEXT } else { TEXT_SECONDARY },
                ))
                .child(div().flex_1().child(entry.label))
                .when_some(entry.count, |item, count| {
                    item.child(Badge::new(count.to_string()).tone(Tone::Count))
                })
                .on_click(move |_, _, cx| nav::navigate(screen, cx));
            column = column.child(item);
            if screen == Screen::Chat && active {
                column = column.child(self.render_sessions(cx));
            }
        }

        column
            .child(section_header())
            .children(RUNNERS.iter().enumerate().map(|(index, runner)| {
                let active = props.current == Some(runner.id);
                let name_color = if runner.created {
                    Some(SPAWN_TEXT)
                } else {
                    None
                };
                let kind_color = if runner.created { SPAWN } else { TEXT_MUTED };
                nav_item(("sidebar-runner", index), active)
                    .child(status_dot(runner.status, false))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .when_some(name_color, |name, color| name.text_color(color))
                                    .child(runner.name),
                            )
                            .child(div().ml_auto().flex().child(icon(
                                runner.kind.icon(),
                                12.,
                                kind_color,
                            ))),
                    )
                    .on_click(Self::runner_click(runner))
            }))
            .child(
                div()
                    .id("sidebar-all")
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(TEXT_LABEL))
                    .text_color(TEXT_MUTED)
                    .cursor_pointer()
                    .hover(|style| style.text_color(TEXT_SECONDARY))
                    .child(data::ALL_SUMMARY)
                    .on_click(|_, _, cx| nav::navigate(Screen::Agents, cx)),
            )
            .child(footer())
    }

    /// Real chat sessions published by the Chat screen (`ui::nav`), plus "New session".
    fn render_sessions(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (sessions, current) = nav::nav_entity(cx)
            .map(|n| {
                let n = n.read(cx);
                (n.chat_sessions.clone(), n.chat_current.clone())
            })
            .unwrap_or_default();
        let rows = sessions.into_iter().enumerate().map(move |(index, session)| {
            let is_current = current.as_ref() == Some(&session.id);
            let status = match session.status {
                nav::ChatStatus::Running => ui::Status::Running,
                nav::ChatStatus::Waiting => ui::Status::Waiting,
                nav::ChatStatus::Error => ui::Status::Error,
                nav::ChatStatus::Idle => ui::Status::Live,
            };
            let id = session.id.clone();
            session_row(("sidebar-session", index), is_current)
                .child(status_dot(status, true))
                .child(ellipsis_cell(session.title))
                .on_click(move |_: &ClickEvent, _, cx| {
                    nav::request_chat(nav::ChatRequest::Open(id.clone()), cx)
                })
        });
        let new_row = session_row("sidebar-session-new", false)
            .text_color(TEXT_MUTED)
            .child(row_icon("plus", 12., TEXT_MUTED))
            .child(ellipsis_cell("New session"))
            .on_click(|_: &ClickEvent, _, cx| nav::request_chat(nav::ChatRequest::New, cx));
        div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .pt(px(2.))
            .pb(px(6.))
            .pl(px(18.))
            .ml(px(15.))
            .border_l_1()
            .border_color(LINE)
            .children(rows)
            .child(new_row)
    }

    // ── Compact rail ────────────────────────────────────────────────────

    fn render_compact(props: &Props) -> gpui::Div {
        let logo = div()
            .id("sidebar-rail-logo")
            .size(px(32.))
            .mb(px(8.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_LG))
            .bg(ACCENT_SOFT)
            .child(icon("pipelines", 16., ACCENT_TEXT))
            .tooltip(tooltip(data::WORKSPACE));

        let nav_buttons = NAV.iter().map(|entry| {
            let active = props.active == Some(entry.screen);
            let screen = entry.screen;
            rail_button(("sidebar-rail-nav", entry.screen as usize), active)
                .child(row_icon(
                    entry.icon,
                    16.,
                    if active { TEXT } else { TEXT_MUTED },
                ))
                .when_some(entry.count, |button, count| button.child(rail_count(count)))
                .tooltip(tooltip(entry.aria))
                .on_click(move |_, _, cx| nav::navigate(screen, cx))
        });

        let runner_buttons = RUNNERS[..RAIL_RUNNERS]
            .iter()
            .enumerate()
            .map(|(index, runner)| {
                let current = props.current == Some(runner.id);
                let (tile_bg, tile_fg) = if runner.created {
                    (SPAWN_SOFT, SPAWN_TEXT)
                } else {
                    runner.kind.tile()
                };
                rail_button(("sidebar-rail-runner", index), current)
                    .child(
                        div()
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_MD))
                            .bg(tile_bg)
                            .border_1()
                            .border_color(if runner.created {
                                SPAWN_LINE
                            } else {
                                gpui::transparent_black()
                            })
                            .when(runner.created, |tile| tile.border_dashed())
                            .child(icon(runner.kind.icon(), 14., tile_fg)),
                    )
                    .child(
                        // 8px dot with a 2px sidebar-colored ring (`box-shadow: 0 0 0 2px`).
                        div()
                            .absolute()
                            .right(px(3.))
                            .bottom(px(3.))
                            .size(px(12.))
                            .rounded_full()
                            .border_2()
                            .border_color(BG_SIDEBAR)
                            .bg(data::rail_dot_color(runner.status)),
                    )
                    .tooltip(tooltip(runner.aria()))
                    .on_click(Self::runner_click(runner))
            });

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .py(px(SPACE_3))
            .px(px(SPACE_2))
            .child(logo)
            .children(nav_buttons)
            .child(div().w(px(24.)).h(px(1.)).my(px(8.)).flex_none().bg(LINE))
            .children(runner_buttons)
            .child(
                rail_button("sidebar-rail-new", false)
                    .child(row_icon("plus", 16., TEXT_MUTED))
                    .tooltip(tooltip(data::NEW_ENTITY))
                    .on_click(|_, _, cx| nav::navigate(Screen::Agents, cx)),
            )
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let props = Self::props(cx);
        let width = if props.compact {
            SIDEBAR_COMPACT_WIDTH
        } else {
            SIDEBAR_WIDTH
        };
        let content = if props.compact {
            Self::render_compact(&props)
        } else {
            self.render_full(&props, cx)
        };
        div()
            .id("app-sidebar")
            .flex_none()
            .w(px(width))
            .h_full()
            .bg(BG_SIDEBAR)
            .border_r_1()
            .border_color(LINE)
            .overflow_y_scroll()
            .child(content)
    }
}

// ── Pieces ──────────────────────────────────────────────────────────────

/// Runner id highlighted on the Agents screen: the shared nav selection (by name).
fn selected_runner_id(cx: &gpui::App) -> &'static str {
    let name = nav::selected_agent(cx);
    let name = name.as_ref().map_or("Payments lead", |name| name.as_ref());
    RUNNERS
        .iter()
        .find(|runner| runner.name == name)
        .map_or("payments-lead", |runner| runner.id)
}

/// Single-line text that ends with "…" when it does not fit. gpui measures nowrap
/// text at its full width, so the text sits in an absolute box sized by the cell.
fn ellipsis_cell(text: impl Into<gpui::SharedString>) -> gpui::Div {
    div().relative().flex_1().min_w_0().h(px(LINE_BODY)).child(
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(text.into()),
    )
}

/// Icon that brightens to `TEXT` when its row (the `ROW_GROUP`) is hovered.
fn row_icon(name: &str, size: f32, color: gpui::Hsla) -> gpui::Svg {
    icon(name, size, color).group_hover(ROW_GROUP, |style| style.text_color(TEXT))
}

/// `.gr-nav-item`: 32px row, 10px gap, secondary text; hover raises, active is selected.
fn nav_item(id: impl Into<ElementId>, active: bool) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .group(ROW_GROUP)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(NAV_ITEM_HEIGHT))
        .px(px(8.))
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .text_color(if active { TEXT } else { TEXT_SECONDARY })
        .when(active, |item| item.bg(SURFACE_HOVER))
        .when(!active, |item| {
            item.hover(|style| style.bg(SURFACE_RAISED).text_color(TEXT))
        })
}

/// 40×40 `.sb-rail` button of the compact rail.
fn rail_button(id: impl Into<ElementId>, current: bool) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .group(ROW_GROUP)
        .relative()
        .size(px(40.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_LG))
        .cursor_pointer()
        .when(current, |button| button.bg(SURFACE_HOVER))
        .when(!current, |button| {
            button.hover(|style| style.bg(SURFACE_RAISED))
        })
}

fn rail_count(count: u32) -> gpui::Div {
    div()
        .absolute()
        .top(px(4.))
        .right(px(4.))
        .min_w(px(14.))
        .h(px(14.))
        .px(px(3.))
        .flex()
        .justify_center()
        .rounded_full()
        .bg(ACCENT_STRONG)
        .text_color(ON_ACCENT)
        .text_size(px(9.))
        .line_height(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(SharedString::from(count.to_string()))
}

fn workspace_row() -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .pt(px(4.))
        .px(px(8.))
        .pb(px(12.))
        .child(
            div()
                .size(px(24.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_MD))
                .bg(ACCENT_SOFT)
                .child(icon("pipelines", 14., ACCENT_TEXT)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(data::WORKSPACE),
        )
        .child(kbd("⌘K"))
}

fn section_header() -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .pt(px(14.))
        .px(px(8.))
        .pb(px(4.))
        .child(
            div()
                .text_size(px(TEXT_LABEL))
                .text_color(TEXT_MUTED)
                .child("Agents & pipelines"),
        )
        .child(
            div()
                .id("sidebar-new")
                .group(ROW_GROUP)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_MD))
                .cursor_pointer()
                .hover(|style| style.bg(SURFACE_RAISED))
                .child(row_icon("plus", 14., TEXT_MUTED))
                .tooltip(tooltip(data::NEW_ENTITY))
                .on_click(|_, _, cx| nav::navigate(Screen::Agents, cx)),
        )
}

fn footer() -> gpui::Div {
    let stat_row = |label: &'static str, value: &'static str| {
        div()
            .flex()
            .justify_between()
            .text_size(px(TEXT_LABEL))
            .child(div().text_color(TEXT_MUTED).child(label))
            .child(div().font_family(FONT_MONO).child(value))
    };
    div()
        .mt(px(16.))
        .pt(px(12.))
        .px(px(8.))
        .pb(px(4.))
        .border_t_1()
        .border_color(LINE)
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(6.))
        .child(stat_row("Agents busy", data::AGENTS_BUSY))
        .child(stat_row("Spend today", data::SPEND_TODAY))
        .child(progress(data::SPEND_FRACTION, ACCENT))
}

/// A session row under the Chat item (28px, hover, current = raised bg).
fn session_row(id: impl Into<gpui::ElementId>, current: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .w_full()
        .min_h(px(28.))
        .py(px(4.))
        .pr(px(8.))
        .pl(px(8.))
        .rounded(px(RADIUS_MD))
        .text_size(px(TEXT_LABEL))
        .text_color(if current { TEXT } else { TEXT_SECONDARY })
        .cursor_pointer()
        .when(current, |row| row.bg(SURFACE_HOVER))
        .when(!current, |row| row.hover(|style| style.bg(SURFACE_RAISED)))
}
