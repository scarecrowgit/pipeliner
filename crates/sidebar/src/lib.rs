//! App sidebar: the full 232px navigation column, or the 56px icon rail on the
//! pipeline editor. Port of `reference/app_sidebar_v4.html`.
//!
//! The nav part is static; the "Chats" part (pinned, recent, "All chats · N") comes from
//! the Chat screen through [`ui::nav`]. "Search chats" and "All chats" open the history
//! picker (⌘K).

mod data;
mod tooltip;

use gpui::{
    ClickEvent, Context, ElementId, FontWeight, IntoElement, Render, SharedString, Stateful,
    Window, div, prelude::*, px,
};
use ui::nav::{self, ChatSessionInfo, ChatStatus, Screen};
use ui::theme::*;
use ui::motion::{self, Look};
use ui::{Badge, Status, Tone, icon, kbd, status_dot};

use data::NAV;
use tooltip::tooltip;

/// Hover group shared by every clickable row, so icons brighten with their row.
const ROW_GROUP: &str = "sidebar-row";

pub struct Sidebar {}

impl Sidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        if let Some(nav) = nav::nav_entity(cx) {
            cx.observe(&nav, |_, _, cx| cx.notify()).detach();
        }
        Self {}
    }

    // ── Full sidebar ────────────────────────────────────────────────────

    fn render_full(&self, screen: Screen, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let in_chat = screen == Screen::Chat;
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(SPACE_3))
            .child(workspace_row());

        for entry in NAV {
            let active = screen == entry.screen;
            let target = entry.screen;
            column = column.child(
                nav_item(("sidebar-nav", entry.screen as usize), active, window, cx)
                    .child(row_icon(
                        entry.icon,
                        16.,
                        if active { TEXT } else { TEXT_SECONDARY },
                    ))
                    .child(div().flex_1().child(entry.label))
                    .when_some(entry.count, |item, count| {
                        item.child(Badge::new(count.to_string()).tone(Tone::Count))
                    })
                    .on_click(move |_, _, cx| nav::navigate(target, cx)),
            );
        }

        let (sessions, current, total) = nav::nav_entity(cx)
            .map(|n| {
                let n = n.read(cx);
                (n.chat_sessions.clone(), n.chat_current.clone(), n.chat_total)
            })
            .unwrap_or_default();
        let current = current.filter(|_| in_chat);
        let now = ui::time::now_secs();
        let offset = ui::time::utc_offset(now);
        let mut pinned = Vec::new();
        let mut recent = Vec::new();
        for s in &sessions {
            let meta = if s.updated > 0 {
                ui::time::when_short(s.updated, now, offset)
            } else {
                String::new()
            };
            // Keyed by chat id so hover/selection fades follow the chat, not the slot.
            let key = ElementId::Name(format!("sidebar-chat-{}", s.id).into());
            let row = chat_row(key, s, meta, current.as_ref() == Some(&s.id), window, cx);
            if s.pinned {
                pinned.push(row);
            } else {
                recent.push(row);
            }
        }

        column = column
            .child(chats_header(in_chat, window, cx))
            .child(search_row(window, cx));
        if !pinned.is_empty() {
            column = column.child(group_label("Pinned", 6.)).children(pinned);
        }
        if !recent.is_empty() {
            column = column.child(group_label("Recent", 10.)).children(recent);
        }
        column.child(
            div()
                .id("sidebar-all-chats")
                .flex_none()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(TEXT_LABEL))
                .text_color(TEXT_MUTED)
                .cursor_pointer()
                .hover(|style| style.text_color(TEXT_SECONDARY))
                .child(SharedString::from(match total {
                    0 => "No chats yet".to_string(),
                    n => format!("All chats · {}", group_thousands(n)),
                }))
                .on_click(|_, _, cx| nav::set_history_open(true, cx)),
        )
    }

    // ── Compact rail ────────────────────────────────────────────────────

    fn render_compact(screen: Screen, window: &mut Window, cx: &mut gpui::App) -> gpui::Div {
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

        let mut nav_buttons = Vec::new();
        for entry in NAV {
            let active = screen == entry.screen;
            let target = entry.screen;
            nav_buttons.push(rail_button(("sidebar-rail-nav", entry.screen as usize), active, window, cx)
                .child(row_icon(
                    entry.icon,
                    16.,
                    if active { TEXT } else { TEXT_MUTED },
                ))
                .when_some(entry.count, |button, count| button.child(rail_count(count)))
                .tooltip(tooltip(entry.aria))
                .on_click(move |_, _, cx| nav::navigate(target, cx)));
        }
        let chats = rail_button("sidebar-rail-chats", screen == Screen::Chat, window, cx)
            .child(row_icon("message", 16., TEXT_MUTED))
            .tooltip(tooltip("Chats"))
            .on_click(|_, _, cx| nav::navigate(Screen::Chat, cx));
        let search = rail_button("sidebar-rail-search", false, window, cx)
            .child(row_icon("search", 16., TEXT_MUTED))
            .tooltip(tooltip("Search chats · ⌘K"))
            .on_click(|_, _, cx| nav::set_history_open(true, cx));

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .py(px(SPACE_3))
            .px(px(SPACE_2))
            .child(logo)
            .children(nav_buttons)
            .child(chats)
            .child(search)
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = nav::current(cx);
        let compact = screen == Screen::PipelineEditor;
        let width = if compact {
            SIDEBAR_COMPACT_WIDTH
        } else {
            SIDEBAR_WIDTH
        };
        let content = if compact {
            Self::render_compact(screen, window, cx)
        } else {
            self.render_full(screen, window, cx)
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

/// `2418` → `2,418`.
fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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

/// Rest / hover looks of a selectable row: selected rows stay on `surface-hover`,
/// others raise on hover. Both fade over `duration-state`.
fn row_looks(active: bool, fg: gpui::Hsla) -> (Look, Look) {
    if active {
        let l = Look::bg(SURFACE_HOVER).fg(TEXT);
        (l, l)
    } else {
        (
            Look::bg(gpui::transparent_black()).fg(fg),
            Look::bg(SURFACE_RAISED).fg(TEXT),
        )
    }
}

/// `.gr-nav-item`: 32px row, 10px gap, secondary text; hover raises, active is selected.
fn nav_item(
    id: impl Into<ElementId>,
    active: bool,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Stateful<gpui::Div> {
    let id = id.into();
    let el = div()
        .id(id.clone())
        .group(ROW_GROUP)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .h(px(NAV_ITEM_HEIGHT))
        .px(px(8.))
        .rounded(px(RADIUS_MD))
        .cursor_pointer();
    let (rest, hover) = row_looks(active, TEXT_SECONDARY);
    motion::hover_fade(el, id, rest, hover, window, cx)
}

/// 40×40 `.sb-rail` button of the compact rail.
fn rail_button(
    id: impl Into<ElementId>,
    current: bool,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Stateful<gpui::Div> {
    let id = id.into();
    let el = div()
        .id(id.clone())
        .group(ROW_GROUP)
        .relative()
        .size(px(40.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_LG))
        .cursor_pointer();
    let (rest, hover) = if current {
        let l = Look::bg(SURFACE_HOVER);
        (l, l)
    } else {
        (Look::bg(gpui::transparent_black()), Look::bg(SURFACE_RAISED))
    };
    motion::hover_fade(el, id, rest, hover, window, cx)
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
}

/// `Chats ………… +`: the label opens the Chat screen, `+` starts a new chat.
fn chats_header(in_chat: bool, window: &mut Window, cx: &mut gpui::App) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .pt(px(16.))
        .px(px(8.))
        .pb(px(6.))
        .child(
            div()
                .id("sidebar-chats")
                .text_size(px(TEXT_LABEL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if in_chat { TEXT } else { TEXT_MUTED })
                .cursor_pointer()
                .hover(|style| style.text_color(TEXT))
                .child("Chats")
                .on_click(|_, _, cx| nav::navigate(Screen::Chat, cx)),
        )
        .child(
            motion::hover_fade(
                div()
                    .id("sidebar-new-chat")
                    .group(ROW_GROUP)
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_MD))
                    .cursor_pointer(),
                "sidebar-new-chat",
                Look::bg(gpui::transparent_black()),
                Look::bg(SURFACE_RAISED),
                window,
                cx,
            )
            .child(row_icon("plus", 14., TEXT_MUTED))
                .tooltip(tooltip("New chat"))
                .on_click(|_, _, cx| nav::request_chat(nav::ChatRequest::New, cx)),
        )
}

/// The "Search chats ⌘K" field-like button.
fn search_row(window: &mut Window, cx: &mut gpui::App) -> Stateful<gpui::Div> {
    let el = div()
        .id("sidebar-search-chats")
        .group(ROW_GROUP)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .px(px(8.))
        .mb(px(6.))
        .rounded(px(RADIUS_LG))
        .border_1()
        .border_color(LINE_STRONG)
        .text_color(TEXT_MUTED)
        .cursor_pointer();
    motion::hover_fade(
        el,
        "sidebar-search-chats",
        Look::bg(SURFACE_RAISED),
        Look::bg(SURFACE_HOVER),
        window,
        cx,
    )
    .child(row_icon("search", 14., TEXT_MUTED))
        .child(div().flex_1().child("Search chats"))
        .child(kbd("⌘K"))
        .on_click(|_, _, cx| nav::set_history_open(true, cx))
}

/// `Pinned` / `Recent` caption.
fn group_label(text: &'static str, top: f32) -> gpui::Div {
    div()
        .flex_none()
        .pt(px(top))
        .px(px(8.))
        .pb(px(2.))
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(TEXT_MUTED)
        .child(text)
}

/// A chat row: status dot · title … when. Opens the chat.
fn chat_row(
    id: impl Into<ElementId>,
    s: &ChatSessionInfo,
    meta: String,
    current: bool,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Stateful<gpui::Div> {
    let status = match s.status {
        ChatStatus::Running => Status::Running,
        ChatStatus::Waiting => Status::Waiting,
        ChatStatus::Error => Status::Error,
        ChatStatus::Idle => Status::Live,
    };
    let chat = s.id.clone();
    let id = id.into();
    let el = div()
        .id(id.clone())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .min_h(px(30.))
        .py(px(4.))
        .px(px(8.))
        .rounded(px(RADIUS_MD))
        .text_size(px(TEXT_LABEL))
        .cursor_pointer();
    let (rest, hover) = row_looks(current, TEXT_SECONDARY);
    motion::hover_fade(el, id, rest, hover, window, cx)
        .child(status_dot(status, true))
        .child(ellipsis_cell(s.title.clone()))
        .when(!meta.is_empty(), |row| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(TEXT_MUTED)
                    .whitespace_nowrap()
                    .child(meta),
            )
        })
        .on_click(move |_: &ClickEvent, _, cx| {
            nav::request_chat(nav::ChatRequest::Open(chat.clone()), cx)
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn thousands() {
        assert_eq!(super::group_thousands(7), "7");
        assert_eq!(super::group_thousands(2418), "2,418");
        assert_eq!(super::group_thousands(1234567), "1,234,567");
    }
}
