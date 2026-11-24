//! Every chat as a list (`Route::Chats`): pinned, today and earlier, with a filter field. The
//! rows follow the sidebar's chat rows: state dot, title, folder, time.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, IntoElement, Render, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use hub::api::{Session, SessionState};
use hub::{ChatIndex, LoadState, Route};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use crate::workdir::common::workdir_label;

/// Width of the list, px.
const COLUMN: f32 = 808.;

/// A group of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Pinned,
    Today,
    Earlier,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Group::Pinned => "Pinned",
            Group::Today => "Today",
            Group::Earlier => "Earlier",
        }
    }
}

/// The dot of a chat's state.
fn status(state: SessionState) -> Status {
    match state {
        SessionState::Idle => Status::Idle,
        SessionState::Running => Status::Running,
        SessionState::Waiting => Status::Waiting,
        SessionState::Failed => Status::Error,
        SessionState::Done => Status::Live,
    }
}

/// The word that goes with the dot.
fn state_word(state: SessionState) -> &'static str {
    match state {
        SessionState::Idle => "idle",
        SessionState::Running => "running",
        SessionState::Waiting => "waiting for you",
        SessionState::Failed => "failed",
        SessionState::Done => "done",
    }
}

/// The whole content of `Route::Chats`: every chat as a list.
pub struct ChatsView {
    index: Entity<ChatIndex>,
    filter: Entity<TextInput>,
    highlight: Option<usize>,
    now: Option<i64>,
    _subs: Vec<Subscription>,
}

impl ChatsView {
    /// The list, loaded from the chat index.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let index = hub::chat_index(cx);
        let filter = cx.new(|cx| TextInput::new(cx).placeholder("Filter chats by title or folder"));
        let subs = vec![
            cx.observe(&index, |_, _, cx| cx.notify()),
            cx.subscribe_in(&filter, window, |this: &mut Self, _, event: &TextInputEvent, _, cx| this.on_filter(event, cx)),
        ];
        let input = filter.clone();
        window.defer(cx, move |window, cx| input.read(cx).focus(window));
        Self { index, filter, highlight: None, now: None, _subs: subs }
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &gpui::App) -> SharedString {
        "Chats".into()
    }

    /// The controls on the right of the shell's header, left to right.
    pub fn header_actions(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            Button::new("chats-new", "New chat")
                .secondary()
                .icon(IconName::Plus)
                .on_click(|_, _, cx| hub::navigate(Route::NewChat, cx))
                .probe("chats-new")
                .into_any_element(),
        ]
    }

    /// Fixes the clock relative times and the today group are read against (tests).
    pub fn set_now(&mut self, now: i64, cx: &mut Context<Self>) {
        self.now = Some(now);
        cx.notify();
    }

    /// The filter field, for tests that type into it.
    #[cfg(test)]
    pub(crate) fn filter(&self) -> &Entity<TextInput> {
        &self.filter
    }

    fn now(&self) -> i64 {
        self.now.unwrap_or_else(base::time::now_ms)
    }

    /// The shown chats in row order, each with its group.
    pub(crate) fn rows(&self, cx: &App) -> Vec<(Group, Arc<Session>)> {
        let index = self.index.read(cx);
        let query = self.filter.read(cx).text().trim().to_lowercase();
        let now = self.now();
        let today = base::time::day_start(now, base::time::utc_offset(now));
        let matches = |session: &Session| {
            if query.is_empty() {
                return true;
            }
            let folder = session
                .workdir
                .as_ref()
                .map(|workdir| workdir_label(&workdir.repo, workdir.branch.as_deref()))
                .unwrap_or_default();
            let hay = format!("{} {folder}", session.title).to_lowercase();
            query.split_whitespace().all(|word| hay.contains(word))
        };
        let mut rows: Vec<(Group, Arc<Session>)> =
            index.pinned().iter().filter(|s| matches(s)).map(|s| (Group::Pinned, s.clone())).collect();
        let (recent, earlier): (Vec<_>, Vec<_>) =
            index.recent().iter().filter(|s| matches(s)).cloned().partition(|s| s.updated >= today);
        rows.extend(recent.into_iter().map(|s| (Group::Today, s)));
        rows.extend(earlier.into_iter().map(|s| (Group::Earlier, s)));
        rows
    }

    fn on_filter(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        let count = self.rows(cx).len();
        match event {
            TextInputEvent::Changed => self.highlight = None,
            TextInputEvent::Down if count > 0 => {
                self.highlight = Some(self.highlight.map_or(0, |ix| (ix + 1).min(count - 1)));
            }
            TextInputEvent::Up if count > 0 => {
                self.highlight = Some(self.highlight.map_or(count - 1, |ix| ix.saturating_sub(1)));
            }
            TextInputEvent::Submit => {
                let ix = self.highlight.unwrap_or(0);
                self.open(ix, cx);
            }
            _ => return,
        }
        cx.notify();
    }

    /// Opens the chat in row `ix`.
    pub(crate) fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some((_, session)) = self.rows(cx).get(ix) {
            hub::navigate(Route::Chat { session: session.id.clone() }, cx);
        }
    }
}

impl Render for ChatsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let index = self.index.read(cx);
        let (state, total, held) = (index.state().clone(), index.total(), index.pinned().len() + index.recent().len());
        let now = self.now();
        let offset = base::time::utc_offset(now);
        let query = self.filter.read(cx).text().trim().to_string();

        let mut list = col().flex_none().gap(px(1.));
        let mut group = None;
        for (ix, (in_group, session)) in rows.iter().enumerate() {
            if group != Some(*in_group) {
                group = Some(*in_group);
                list = list.child(group_head(*in_group));
            }
            let current = self.highlight == Some(ix);
            list = list.child(chat_row(ix, session, current, now, offset, window, cx));
        }
        let empty = match (&state, rows.is_empty()) {
            (LoadState::Loading, _) => Some("Loading chats…".to_string()),
            (LoadState::Failed(error), _) => Some(format!("Could not read the chats: {error}")),
            (LoadState::Ready, true) if query.is_empty() => Some("No chats yet".to_string()),
            (LoadState::Ready, true) => Some(format!("No chats match “{query}”")),
            _ => None,
        };
        let foot = (state == LoadState::Ready && total > held as u64)
            .then(|| format!("Showing the {held} latest of {} chats", base::fmt::count(total)));

        div().id("chats-scroll").size_full().bg(BG).overflow_y_scroll().child(
            col().w_full().items_center().child(
                col()
                    .w_full()
                    .max_w(px(COLUMN))
                    .px(px(SPACE_6))
                    .py(px(SPACE_6))
                    .child(text_field("chats-filter", &self.filter).icon(IconName::Search).probe("chats-filter"))
                    .child(div().mt(px(SPACE_2)).child(list))
                    .when_some(empty, |d, text| {
                        d.child(div().mt(px(SPACE_4)).px(px(SPACE_2)).text_color(TEXT_MUTED).child(text))
                    })
                    .when_some(foot, |d, text| d.child(div().mt(px(SPACE_4)).px(px(SPACE_2)).child(Note::new(text)))),
            ),
        )
    }
}

/// `.sb-sub`: "Pinned", "Today", "Earlier".
fn group_head(group: Group) -> impl IntoElement {
    row()
        .flex_none()
        .gap(px(SPACE_1_5))
        .h(px(28.))
        .pt(px(SPACE_2))
        .px(px(SPACE_2))
        .type_style(CAPTION)
        .text_color(TEXT_MUTED)
        .when(group == Group::Pinned, |d| d.child(icon(IconName::Pin, 12., TEXT_MUTED)))
        .child(group.title())
}

/// A chat row (`.li`): state dot, title, the state word when someone is needed, the folder,
/// the time. Click opens it.
fn chat_row(
    ix: usize,
    session: &Session,
    current: bool,
    now: i64,
    offset: i32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let name: SharedString = format!("chats-row-{ix}").into();
    let (rest, hover) = if current {
        let look = Look::bg(SURFACE_HOVER).fg(TEXT);
        (look, look)
    } else {
        (Look::bg(TRANSPARENT).fg(TEXT_SECONDARY), Look::bg(SURFACE_RAISED).fg(TEXT))
    };
    let faded = motion::hover_fade_state(name.clone(), rest, hover, window, cx);
    let selector = name.to_string();
    let id = session.id.clone();
    let folder = session.workdir.as_ref().map(|workdir| workdir_label(&workdir.repo, workdir.branch.as_deref()));
    let needs_word = matches!(session.state, SessionState::Waiting | SessionState::Failed | SessionState::Running);
    let el = row()
        .id(ElementId::Name(name.clone()))
        .debug_selector(move || selector)
        .flex_none()
        .gap(px(SPACE_2))
        .h(px(36.))
        .px(px(SPACE_2))
        .rounded(px(RADIUS_MD))
        .type_style(BODY)
        .cursor_pointer()
        .on_click(move |_, _, cx| hub::navigate(Route::Chat { session: id.clone() }, cx));
    faded
        .apply(el)
        .child(div().mx(px(1.)).child(StatusDot::new(status(session.state))))
        .child(truncate(session.title.clone()))
        .when(needs_word, |d| {
            let tone = if session.state == SessionState::Running { TEXT_MUTED } else { WARNING };
            d.child(div().flex_none().type_style(LABEL).text_color(tone).child(state_word(session.state)))
        })
        .child(
            div()
                .flex_none()
                .w(px(240.))
                .type_style(MONO_SM)
                .text_color(TEXT_MUTED)
                .child(crate::workdir::common::path_line(folder.unwrap_or_else(|| "no folder".to_string()), MONO_SM.line)),
        )
        .child(
            div()
                .flex_none()
                .w(px(48.))
                .flex()
                .justify_end()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .tabular()
                .whitespace_nowrap()
                .child(base::time::ago(now, session.updated, offset)),
        )
        .probe(name)
        .into_any_element()
}
