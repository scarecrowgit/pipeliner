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
            .child(workspace_row())
            .child(chat_actions(window, cx));

        let (sessions, current, total) = nav::nav_entity(cx)
            .map(|n| {
                let n = n.read(cx);
                (n.chat_sessions.clone(), n.chat_current.clone(), n.chat_total)
            })
            .unwrap_or_default();
        let waiting = sessions
            .iter()
            .filter(|s| s.status == ChatStatus::Waiting)
            .count();

        let inbox = inbox_count(cx);
        for entry in NAV {
            let active = screen == entry.screen;
            let target = entry.screen;
            let count = if entry.screen == Screen::Inbox { inbox } else { entry.count };
            column = column.child(
                nav_item(("sidebar-nav", entry.screen as usize), active, window, cx)
                    .child(row_icon(
                        entry.icon,
                        16.,
                        if active { TEXT } else { TEXT_SECONDARY },
                    ))
                    .child(div().flex_1().child(entry.label))
                    .when_some(count, |item, count| {
                        item.child(Badge::new(count.to_string()).tone(Tone::Count))
                    })
                    .on_click(move |_, _, cx| nav::navigate(target, cx)),
            );
            // "Chats" sits under Inbox: it opens the chat picker; active on the Chat screen.
            if entry.screen == Screen::Inbox {
                column = column.child(
                    nav_item("sidebar-nav-chats", in_chat, window, cx)
                        .child(row_icon(
                            "message",
                            16.,
                            if in_chat { TEXT } else { TEXT_SECONDARY },
                        ))
                        .child(div().flex_1().child("Chats"))
                        .when(waiting > 0, |item| {
                            item.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(WARNING)
                                    .child(format!("{waiting} waiting")),
                            )
                        })
                        .on_click(|_, _, cx| nav::set_history_open(true, cx)),
                );
            }
        }

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

        column = column.child(recent_header(total));
        if !pinned.is_empty() {
            column = column.child(group_label("Pinned", 6.)).children(pinned);
        }
        if !recent.is_empty() {
            column = column.child(group_label("Recent", 10.)).children(recent);
        }
        if total == 0 {
            return column.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(TEXT_LABEL))
                    .text_color(TEXT_MUTED)
                    .child("No chats yet"),
            );
        }
        let find = div()
            .id("sidebar-find-older")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .mt(px(4.))
            .px(px(8.))
            .rounded(px(RADIUS_MD))
            .text_color(ACCENT_TEXT)
            .cursor_pointer();
        column.child(
            motion::hover_fade(
                find,
                "sidebar-find-older",
                Look::bg(gpui::transparent_black()),
                Look::bg(SURFACE_RAISED),
                window,
                cx,
            )
            .child(icon("search", 14., ACCENT_TEXT))
            .child("Find an older chat")
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

        let inbox = inbox_count(cx);
        let new_chat = {
            let el = div()
                .id("sidebar-rail-new")
                .relative()
                .size(px(40.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_LG))
                .cursor_pointer();
            motion::hover_fade(
                el,
                "sidebar-rail-new",
                Look::bg(ACCENT_STRONG),
                Look::bg(ACCENT_STRONG_HOVER),
                window,
                cx,
            )
            .child(icon("plus", 16., ON_ACCENT))
            .tooltip(tooltip("New chat · ⌘N"))
            .on_click(|_, _, cx| nav::request_chat(nav::ChatRequest::New, cx))
        };
        let open_chat = rail_button("sidebar-rail-open", false, window, cx)
            .child(row_icon("message", 16., TEXT_MUTED))
            .tooltip(tooltip("Open a chat · ⌘K"))
            .on_click(|_, _, cx| nav::set_history_open(true, cx));
        let mut nav_buttons = vec![new_chat, open_chat];
        for entry in NAV {
            let active = screen == entry.screen;
            let target = entry.screen;
            let count = if entry.screen == Screen::Inbox { inbox } else { entry.count };
            nav_buttons.push(rail_button(("sidebar-rail-nav", entry.screen as usize), active, window, cx)
                .child(row_icon(
                    entry.icon,
                    16.,
                    if active { TEXT } else { TEXT_MUTED },
                ))
                .when_some(count, |button, count| button.child(rail_count(count)))
                .tooltip(tooltip(entry.aria))
                .on_click(move |_, _, cx| nav::navigate(target, cx)));
        }
        let chats = rail_button("sidebar-rail-chats", screen == Screen::Chat, window, cx)
            .child(row_icon("search", 16., TEXT_MUTED))
            .tooltip(tooltip("Chats"))
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

/// Inbox badge: reports and reviews waiting (published by the Inbox screen).
fn inbox_count(cx: &gpui::App) -> Option<u32> {
    let n = nav::nav_entity(cx).map_or(0, |n| n.read(cx).inbox_count);
    (n > 0).then_some(n as u32)
}

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

/// "New chat ⌘N" (primary) and "Open a chat… ⌘K" at the top of the sidebar.
fn chat_actions(window: &mut Window, cx: &mut gpui::App) -> gpui::Div {
    let base = |id: &'static str| {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(36.))
            .px(px(10.))
            .rounded(px(RADIUS_LG))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
    };
    let new_chat = motion::hover_fade(
        base("sidebar-new-chat"),
        "sidebar-new-chat",
        Look::bg(ACCENT_STRONG).fg(ON_ACCENT),
        Look::bg(ACCENT_STRONG_HOVER).fg(ON_ACCENT),
        window,
        cx,
    )
    .child(icon("plus", 14., ON_ACCENT))
    .child(div().flex_1().child("New chat"))
    .child(
        div()
            .font_family(FONT_MONO)
            .text_size(px(11.))
            .text_color(ON_ACCENT.opacity(0.78))
            .child("⌘N"),
    )
    .on_click(|_, _, cx| nav::request_chat(nav::ChatRequest::New, cx));
    let open_chat = motion::hover_fade(
        base("sidebar-open-chat").border_1(),
        "sidebar-open-chat",
        Look::bg(SURFACE_RAISED).fg(TEXT).border(LINE_STRONG),
        Look::bg(SURFACE_HOVER).fg(TEXT).border(LINE_STRONG),
        window,
        cx,
    )
    .child(icon("message", 14., TEXT_SECONDARY))
    .child(div().flex_1().child("Open a chat…"))
    .child(kbd("⌘K"))
    .on_click(|_, _, cx| nav::set_history_open(true, cx));
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(6.))
        .pb(px(12.))
        .child(new_chat)
        .child(open_chat)
}

/// `Recent chats ………… View all N`.
fn recent_header(total: usize) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .pt(px(16.))
        .px(px(8.))
        .pb(px(4.))
        .text_size(px(12.))
        .child(div().text_color(TEXT_MUTED).child("Recent chats"))
        .when(total > 0, |d| {
            d.child(
                div()
                    .id("sidebar-view-all")
                    .text_color(ACCENT_TEXT)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(SharedString::from(format!(
                        "View all {}",
                        group_thousands(total)
                    )))
                    .on_click(|_, _, cx| nav::set_history_open(true, cx)),
            )
        })
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
        .min_h(px(32.))
        .py(px(4.))
        .px(px(8.))
        .rounded(px(RADIUS_MD))
        .text_size(px(TEXT_BODY))
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
