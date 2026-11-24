//! The Chats section of the sidebar: pinned and recent chats, the open chat's sub-agents under
//! it, and the hover actions of a row (pin or unpin, archive).

use std::sync::Arc;

use gpui::{AnyElement, App, ClickEvent, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::Route;
use model::{Session, SessionId, SessionState};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use super::{RECENT_ROWS, Sidebar, section_head, sub_head};

/// The dot of a chat's state.
pub(crate) fn status(state: SessionState) -> Status {
    match state {
        SessionState::Idle => Status::Idle,
        SessionState::Running => Status::Running,
        SessionState::Waiting => Status::Waiting,
        SessionState::Failed => Status::Error,
        SessionState::Done => Status::Live,
    }
}

/// The word that goes with the dot (`DOT_LABEL` of the mockups).
pub(crate) fn state_word(state: SessionState) -> &'static str {
    match state {
        SessionState::Idle => "idle",
        SessionState::Running => "running",
        SessionState::Waiting => "waiting for you",
        SessionState::Failed => "failed",
        SessionState::Done => "done",
    }
}

/// The probe and element name of a chat row.
pub(crate) fn row_name(id: &SessionId) -> SharedString {
    format!("sidebar-chat-{id}").into()
}

impl Sidebar {
    pub(super) fn render_chats(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let index = self.chats.read(cx);
        let pinned: Vec<Arc<Session>> = index.pinned().to_vec();
        let recent: Vec<Arc<Session>> = index.recent().iter().take(RECENT_ROWS).cloned().collect();
        let children: Vec<Arc<Session>> = index.children().to_vec();
        let total = index.total();
        let ready = index.state() == &hub::LoadState::Ready;
        let open = self.open_chat(cx);
        let now = base::time::now_ms();
        let offset = base::time::utc_offset(now);

        let mut shown = Vec::new();
        let mut list = col().flex_none().gap(px(1.));
        let mut add = |list: gpui::Div, session: &Arc<Session>, window: &mut Window, cx: &mut App| {
            shown.push((session.id.clone(), session.state));
            let current = open.as_ref() == Some(&session.id);
            let mut list = list.child(chat_row(session, current, now, offset, window, cx));
            if current {
                for child in &children {
                    shown.push((child.id.clone(), child.state));
                    list = list.child(child_row(child));
                }
            }
            list
        };
        if !pinned.is_empty() {
            list = list.child(sub_head("Pinned", true));
            for session in &pinned {
                list = add(list, session, window, cx);
            }
        }
        if !recent.is_empty() {
            list = list.child(sub_head("Recent", false));
            for session in &recent {
                list = add(list, session, window, cx);
            }
        }
        if ready && pinned.is_empty() && recent.is_empty() {
            list = list.child(
                div().flex_none().h(px(LIST_ITEM_HEIGHT)).px(px(SPACE_2)).text_color(TEXT_MUTED).child("No chats yet"),
            );
        }
        self.shown.chats = shown;

        col()
            .flex_none()
            .mt(px(SPACE_4))
            .gap(px(1.))
            .child(section_head(
                "Chats",
                "sidebar-chats-all",
                format!("View all {}", base::fmt::count(total)),
                Route::Chats,
                window,
                cx,
            ))
            .child(list)
            .into_any_element()
    }
}

/// A chat row (`.li`): state dot, title on one line, time; on hover the time gives way to
/// pin and archive buttons.
fn chat_row(session: &Session, current: bool, now: i64, offset: i32, window: &mut Window, cx: &mut App) -> AnyElement {
    let name = row_name(&session.id);
    let (rest, hover) = if current {
        let look = Look::bg(SURFACE_HOVER).fg(TEXT);
        (look, look)
    } else {
        (Look::bg(TRANSPARENT).fg(TEXT_SECONDARY), Look::bg(SURFACE_RAISED).fg(TEXT))
    };
    let faded = motion::hover_fade_state(name.clone(), rest, hover, window, cx);
    let hovered = motion::hovered(name.clone(), window, cx);
    let selector = name.to_string();
    let id = session.id.clone();
    let el = row()
        .id(ElementId::Name(name.clone()))
        .debug_selector(move || selector)
        .flex_none()
        .gap(px(SPACE_2))
        .h(px(LIST_ITEM_HEIGHT))
        .px(px(SPACE_2))
        .rounded(px(RADIUS_MD))
        .type_style(BODY)
        .cursor_pointer()
        .on_click(move |_, _, cx| hub::navigate(Route::Chat { session: id.clone() }, cx));
    let trailing = if hovered {
        row_actions(session, &name).into_any_element()
    } else {
        div()
            .flex_none()
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .tabular()
            .whitespace_nowrap()
            .child(base::time::ago(now, session.updated, offset))
            .into_any_element()
    };
    faded
        .apply(el)
        .child(div().mx(px(1.)).child(StatusDot::new(status(session.state))))
        .child(truncate(session.title.clone()))
        .child(trailing)
        .probe(name)
        .into_any_element()
}

/// Pin or unpin, and archive (with Undo).
fn row_actions(session: &Session, name: &SharedString) -> impl IntoElement {
    let (pin_id, archive_id): (SharedString, SharedString) = (format!("{name}-pin").into(), format!("{name}-archive").into());
    let pinned = session.pinned;
    let id = session.id.clone();
    let pin = IconButton::new(ElementId::Name(pin_id.clone()), IconName::Pin)
        .small()
        .tooltip(if pinned { "Unpin" } else { "Pin" })
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            hub::commands(cx).pin_session(&id, !pinned, cx);
        })
        .probe(pin_id);
    let id = session.id.clone();
    let title: SharedString = session.title.clone().into();
    let archive = IconButton::new(ElementId::Name(archive_id.clone()), IconName::Archive)
        .small()
        .tooltip("Archive")
        .on_click(move |_: &ClickEvent, window, cx| {
            cx.stop_propagation();
            archive_chat(&id, title.clone(), window, cx);
        })
        .probe(archive_id);
    row().flex_none().child(pin).child(archive)
}

/// Archive a chat and offer Undo.
fn archive_chat(id: &SessionId, title: SharedString, window: &mut Window, cx: &mut App) {
    let commands = hub::commands(cx);
    commands.archive_session(id, true, cx);
    let id = id.clone();
    let toast = Toast::new("Chat archived").detail(title).undo(move |_, cx| {
        commands.archive_session(&id, false, cx);
    });
    ui::overlays(window, cx).toast(toast, cx);
}

/// A sub-agent of the open chat (`.li.child`): violet, on the spawn guide.
fn child_row(session: &Session) -> impl IntoElement {
    let name = row_name(&session.id);
    let id = session.id.clone();
    let label = format!("@{} · {}", session.runnable.id(), session.title);
    ListItem::new(ElementId::Name(name.clone()), label)
        .spawned()
        .meta(state_word(session.state))
        .on_click(move |_, _, cx| hub::navigate(Route::Chat { session: id.clone() }, cx))
        .probe(name)
}
