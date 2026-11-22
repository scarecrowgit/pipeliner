//! Chat history picker (⌘K): every saved chat, searchable by title, message text,
//! files and folder, with `in:` / `agent:` / `is:` / `has:` filter chips, quick status
//! filters, time groups, a preview of the selected chat and pin / archive / fork /
//! export / delete. Port of `reference/chat_history_picker.html`.
//!
//! The app shell renders it over every screen while `nav.history_open` is set.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable, FontWeight,
    Hsla, IntoElement, KeyBinding, KeyDownEvent, Render, ScrollHandle, SharedString, StyledText,
    Task, TextRun, Window, actions, div, font, prelude::*, px, relative,
};
use ui::nav::{self, ChatStatus};
use ui::{Button, Status, TextInput, TextInputEvent, icon, kbd, status_dot, theme::*};

use crate::history_index::{
    self as hx, ChatMeta, Filter, Flags, Group, LiveStatus, Scope, Snippet, Sort,
};
use crate::session;

actions!(chat_history, [RemoveLastToken, ArchiveSelected, TogglePinSelected]);

/// Rows shown per group before "Load N more".
const PAGE: usize = 50;
const CONTEXT: &str = "ChatHistory";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        // Reached only when the search field has nothing to delete (it propagates).
        KeyBinding::new("backspace", RemoveLastToken, Some(CONTEXT)),
        KeyBinding::new("ctrl-backspace", ArchiveSelected, Some(CONTEXT)),
        KeyBinding::new("cmd-backspace", ArchiveSelected, Some(CONTEXT)),
        KeyBinding::new("ctrl-p", TogglePinSelected, Some(CONTEXT)),
        KeyBinding::new("cmd-p", TogglePinSelected, Some(CONTEXT)),
    ]);
}

struct Toast {
    text: String,
    /// Archived chats "Undo" restores (empty: nothing to undo).
    undo: Vec<String>,
}

pub struct ChatHistory {
    open: bool,
    query: Entity<TextInput>,
    filter: Filter,
    chats: Vec<ChatMeta>,
    flags: Flags,
    loading: bool,
    loaded_at: u64,
    /// Selected (previewed) chat.
    sel: Option<String>,
    /// Group open/closed overrides of the defaults.
    group_open: HashMap<Group, bool>,
    /// Rows shown per group (grows with "Load more").
    limits: HashMap<Group, usize>,
    selecting: bool,
    checked: BTreeSet<String>,
    confirm_delete: bool,
    sort_open: bool,
    toast: Option<Toast>,
    /// Live statuses published by the Chat screen (the open chat may be running).
    live: HashMap<SharedString, ChatStatus>,
    current: Option<SharedString>,
    host: String,
    focus: FocusHandle,
    restore_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
    load_task: Option<Task<()>>,
    toast_task: Option<Task<()>>,
    _subs: Vec<gpui::Subscription>,
}

impl ChatHistory {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
        let query = cx.new(|cx| {
            TextInput::new(
                "Search titles, messages and files · in:folder agent:name is:waiting has:files",
                cx,
            )
        });
        let mut subs = vec![cx.subscribe_in(&query, window, Self::on_query_event)];
        if let Some(nav) = nav::nav_entity(cx) {
            subs.push(cx.observe_in(&nav, window, |this, nav, window, cx| {
                let n = nav.read(cx);
                this.live = n
                    .chat_sessions
                    .iter()
                    .map(|s| (s.id.clone(), s.status))
                    .collect();
                this.current = n.chat_current.clone();
                let open = n.history_open;
                if open != this.open {
                    this.open = open;
                    if open {
                        this.on_open(window, cx);
                    } else {
                        this.on_close(window, cx);
                    }
                }
                cx.notify();
            }));
        }
        // Dev/test hook: `HARNESS_CHAT_HISTORY=1` opens the picker on start;
        // `HARNESS_CHAT_HISTORY_Q` pre-fills its query (`in:x retry`).
        if std::env::var("HARNESS_CHAT_HISTORY").is_ok_and(|v| v == "1") {
            cx.defer(|cx| nav::set_history_open(true, cx));
        }
        Self {
            open: false,
            query,
            filter: Filter::default(),
            chats: Vec::new(),
            flags: Flags::default(),
            loading: false,
            loaded_at: 0,
            sel: None,
            group_open: HashMap::new(),
            limits: HashMap::new(),
            selecting: false,
            checked: BTreeSet::new(),
            confirm_delete: false,
            sort_open: false,
            toast: None,
            live: HashMap::new(),
            current: None,
            host: crate::workdir::hostname(),
            focus: cx.focus_handle(),
            restore_focus: None,
            scroll: ScrollHandle::new(),
            load_task: None,
            toast_task: None,
            _subs: subs,
        }
    }

    // ── Open / close / load ─────────────────────────────────────────────

    fn on_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_focus = window.focused(cx);
        self.filter = Filter::default();
        self.query.update(cx, |q, cx| q.set_text("", cx));
        self.group_open.clear();
        self.limits.clear();
        self.selecting = false;
        self.checked.clear();
        self.confirm_delete = false;
        self.sort_open = false;
        self.toast = None;
        self.sel = self.current.as_ref().map(|s| s.to_string());
        if let Ok(q) = std::env::var("HARNESS_CHAT_HISTORY_Q") {
            for word in q.split_whitespace() {
                if hx::split_trailing_token(&format!("{word} ")).is_some() {
                    self.filter.tokens.push(word.to_string());
                } else {
                    self.filter.query = word.to_string();
                }
            }
            let text = self.filter.query.clone();
            self.query.update(cx, |q, cx| q.set_text(text, cx));
        }
        self.query.focus_handle(cx).focus(window);
        self.reload(cx);
    }

    fn on_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(h) = self.restore_focus.take() {
            h.focus(window);
        }
        self.toast = None;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.defer(|cx| nav::set_history_open(false, cx));
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let (chats, flags) = cx
                .background_spawn(async {
                    (hx::scan(&session::sessions_dir()), Flags::load())
                })
                .await;
            this.update(cx, |this, cx| {
                this.chats = chats;
                this.flags = flags;
                this.loading = false;
                this.loaded_at = hx::now_secs();
                cx.notify();
            })
            .ok();
        }));
    }

    /// Flags changed: save them and tell the Chat screen (sidebar list) to reload.
    fn save_flags(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.flags.save() {
            self.show_toast(format!("Could not save: {e}"), Vec::new(), cx);
        }
        cx.defer(nav::chats_changed);
        cx.notify();
    }

    fn show_toast(&mut self, text: String, undo: Vec<String>, cx: &mut Context<Self>) {
        self.toast = Some(Toast { text, undo });
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(8)).await;
            this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ── Query ───────────────────────────────────────────────────────────

    fn on_query_event(
        &mut self,
        input: &Entity<TextInput>,
        ev: &TextInputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            TextInputEvent::Changed => {
                let text = input.read(cx).text().to_string();
                if let Some((rest, token)) = hx::split_trailing_token(&text) {
                    if !self.filter.tokens.contains(&token) {
                        self.filter.tokens.push(token);
                    }
                    input.update(cx, |q, cx| q.set_text(rest.clone(), cx));
                    self.filter.query = rest;
                } else {
                    self.filter.query = text;
                }
                self.limits.clear();
                cx.notify();
            }
            TextInputEvent::Up => self.move_sel(-1, cx),
            TextInputEvent::Down => self.move_sel(1, cx),
            TextInputEvent::Submit => self.open_selected(cx),
            TextInputEvent::Cancel => self.close(cx),
            _ => {}
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "escape" => self.close(cx),
            "up" => self.move_sel(-1, cx),
            "down" => self.move_sel(1, cx),
            "enter" => self.open_selected(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn remove_last_token(&mut self, _: &RemoveLastToken, _: &mut Window, cx: &mut Context<Self>) {
        if self.filter.tokens.pop().is_some() {
            cx.notify();
        }
    }

    // ── Derived view ────────────────────────────────────────────────────

    fn status_of(&self, id: &str) -> LiveStatus {
        match self.live.get(id) {
            Some(ChatStatus::Waiting) => LiveStatus::Waiting,
            Some(ChatStatus::Running) => LiveStatus::Running,
            Some(ChatStatus::Error) => LiveStatus::Failed,
            _ => LiveStatus::Done,
        }
    }

    fn group_key(&self, c: &ChatMeta, now: u64, offset: i64) -> Group {
        if self.flags.is_pinned(&c.id) {
            Group::Pinned
        } else {
            hx::group_of(c.updated, now, offset)
        }
    }

    fn groups(&self) -> Vec<GroupView<'_>> {
        let now = hx::now_secs();
        let offset = hx::utc_offset(now);
        let filtering = self.filter.is_filtering();
        let shown = hx::filter(&self.chats, &self.flags, &self.filter, &|c| {
            self.status_of(&c.id)
        });
        let mut totals: HashMap<Group, usize> = HashMap::new();
        for c in self.chats.iter().filter(|c| !self.flags.is_archived(&c.id)) {
            *totals.entry(self.group_key(c, now, offset)).or_default() += 1;
        }
        let mut by_group: HashMap<Group, Vec<&ChatMeta>> = HashMap::new();
        for c in shown {
            by_group
                .entry(self.group_key(c, now, offset))
                .or_default()
                .push(c);
        }
        let mut keys: Vec<Group> = totals.keys().copied().collect();
        keys.sort_by_key(|g| group_rank(*g));
        keys.into_iter()
            .filter_map(|key| {
                let rows = by_group.remove(&key).unwrap_or_default();
                let total = totals.get(&key).copied().unwrap_or(0);
                if self.filter.scope == Scope::Agents || (filtering && rows.is_empty()) {
                    return None;
                }
                let open = self
                    .group_open
                    .get(&key)
                    .copied()
                    .unwrap_or(filtering || !key.folded_by_default());
                let limit = self.limits.get(&key).copied().unwrap_or(PAGE);
                let more = rows.len().saturating_sub(limit);
                let cost: f64 = rows.iter().map(|c| c.cost_usd).sum();
                let count = if key == Group::Pinned || !filtering {
                    rows.len().to_string()
                } else {
                    format!("{} of {total}", rows.len())
                };
                let rows = if open { rows.into_iter().take(limit).collect() } else { Vec::new() };
                Some(GroupView { key, count, cost, open, more, rows })
            })
            .collect()
    }

    /// Ids of the rows on screen, top to bottom (keyboard order).
    fn order(&self) -> Vec<String> {
        self.groups()
            .iter()
            .flat_map(|g| g.rows.iter().map(|c| c.id.clone()))
            .collect()
    }

    fn selected(&self) -> Option<&ChatMeta> {
        let order = self.order();
        let id = self
            .sel
            .as_ref()
            .filter(|s| order.contains(s))
            .or(order.first())?
            .clone();
        self.chats.iter().find(|c| c.id == id)
    }

    fn move_sel(&mut self, delta: isize, cx: &mut Context<Self>) {
        let order = self.order();
        if order.is_empty() {
            return;
        }
        let cur = self
            .sel
            .as_ref()
            .and_then(|s| order.iter().position(|o| o == s));
        let next = match cur {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, order.len() as isize - 1) as usize,
        };
        self.sel = Some(order[next].clone());
        if let Some(ix) = self.row_child_index(&order[next]) {
            self.scroll.scroll_to_item(ix);
        }
        cx.notify();
    }

    /// Index of a row among the list's direct children (group headers count).
    fn row_child_index(&self, id: &str) -> Option<usize> {
        let mut ix = 0;
        for g in self.groups() {
            ix += 1;
            for c in &g.rows {
                if c.id == id {
                    return Some(ix);
                }
                ix += 1;
            }
            if g.more > 0 {
                ix += 1;
            }
        }
        None
    }

    // ── Actions ─────────────────────────────────────────────────────────

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selected().map(|c| c.id.clone()) {
            self.open_chat(id, cx);
        }
    }

    fn open_chat(&mut self, id: String, cx: &mut Context<Self>) {
        cx.defer(move |cx| {
            nav::set_history_open(false, cx);
            nav::request_chat(nav::ChatRequest::Open(id.into()), cx);
        });
    }

    fn fork_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selected().map(|c| c.id.clone()) {
            cx.defer(move |cx| {
                nav::set_history_open(false, cx);
                nav::request_chat(nav::ChatRequest::Fork(id.into()), cx);
            });
        }
    }

    fn toggle_pin(&mut self, _: &TogglePinSelected, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected().map(|c| c.id.clone()) else {
            return;
        };
        if !self.flags.pinned.remove(&id) {
            self.flags.pinned.insert(id.clone());
        }
        self.sel = Some(id);
        self.save_flags(cx);
    }

    fn archive_selected(&mut self, _: &ArchiveSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected().map(|c| c.id.clone()) {
            self.archive(vec![id], cx);
        }
    }

    fn archive(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        // Keep the selection near where it was.
        let order = self.order();
        let next = self
            .sel
            .as_ref()
            .and_then(|s| order.iter().position(|o| o == s))
            .and_then(|i| order.iter().skip(i).find(|o| !ids.contains(o)).cloned());
        for id in &ids {
            self.flags.archived.insert(id.clone());
            self.checked.remove(id);
        }
        self.sel = next;
        let text = match ids.len() {
            1 => "1 chat archived".to_string(),
            n => format!("{n} chats archived"),
        };
        self.save_flags(cx);
        self.show_toast(text, ids, cx);
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.toast.take() else {
            return;
        };
        for id in &t.undo {
            self.flags.archived.remove(id);
        }
        self.sel = t.undo.first().cloned();
        self.save_flags(cx);
    }

    fn export_checked(&mut self, cx: &mut Context<Self>) {
        let ids = self.checked.clone();
        let chats: Vec<ChatMeta> = self
            .chats
            .iter()
            .filter(|c| ids.contains(&c.id))
            .cloned()
            .collect();
        if chats.is_empty() {
            return;
        }
        let path = export_path();
        let n = chats.len();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let path = path.clone();
                    async move {
                        let refs: Vec<&ChatMeta> = chats.iter().collect();
                        let md = hx::export_markdown(&refs, &session::sessions_dir());
                        std::fs::write(&path, md)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let text = match result {
                    Ok(()) => format!(
                        "Exported {n} chat{} to {}",
                        if n == 1 { "" } else { "s" },
                        crate::workdir::tilde(&path, home().as_deref())
                    ),
                    Err(e) => format!("Export failed: {e}"),
                };
                this.show_toast(text, Vec::new(), cx);
            })
            .ok();
        })
        .detach();
    }

    fn delete_checked(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.checked.iter().cloned().collect();
        let dir = session::sessions_dir();
        let mut failed = 0;
        for id in &ids {
            if id.contains('/') || id.contains("..") {
                continue;
            }
            if std::fs::remove_file(dir.join(format!("{id}.json"))).is_err() {
                failed += 1;
            }
            self.flags.pinned.remove(id);
            self.flags.archived.remove(id);
        }
        self.chats.retain(|c| !ids.contains(&c.id));
        self.checked.clear();
        self.confirm_delete = false;
        let text = match (ids.len() - failed, failed) {
            (n, 0) => format!("Deleted {n} chat{}", if n == 1 { "" } else { "s" }),
            (n, f) => format!("Deleted {n}, {f} could not be removed"),
        };
        self.save_flags(cx);
        self.show_toast(text, Vec::new(), cx);
    }
}

struct GroupView<'a> {
    key: Group,
    count: String,
    cost: f64,
    open: bool,
    /// Matching rows not shown yet ("Load more").
    more: usize,
    rows: Vec<&'a ChatMeta>,
}

/// Pinned, Today, …, then months newest first.
fn group_rank(g: Group) -> (u8, i64) {
    match g {
        Group::Pinned => (0, 0),
        Group::Today => (1, 0),
        Group::Yesterday => (2, 0),
        Group::ThisWeek => (3, 0),
        Group::ThisMonth => (4, 0),
        Group::Month(y, m) => (5, -(i64::from(y) * 12 + i64::from(m))),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `~/Downloads/harness-chats-<unix time>.md` (home when there is no Downloads).
fn export_path() -> PathBuf {
    let home = home().unwrap_or_else(std::env::temp_dir);
    let downloads = home.join("Downloads");
    let dir = if downloads.is_dir() { downloads } else { home };
    dir.join(format!("harness-chats-{}.md", hx::now_secs()))
}

fn eid(name: &'static str, ix: usize) -> ElementId {
    ElementId::NamedInteger(name.into(), ix as u64)
}

fn live_view(st: LiveStatus) -> (Status, &'static str, Hsla) {
    match st {
        LiveStatus::Waiting => (Status::Waiting, "waiting for you", WARNING),
        LiveStatus::Running => (Status::Running, "running", INFO),
        LiveStatus::Failed => (Status::Error, "failed", DANGER),
        LiveStatus::Done => (Status::Live, "done", TEXT_MUTED),
    }
}

/// Text with the first match of `q` highlighted (accent on soft accent).
fn highlighted(text: &str, q: &str, weight: FontWeight, color: Hsla) -> StyledText {
    let run = |len: usize, color: Hsla, bg: Option<Hsla>| {
        let mut f = font(FONT_SANS);
        f.weight = weight;
        TextRun {
            len,
            font: f,
            color,
            background_color: bg,
            underline: None,
            strikethrough: None,
        }
    };
    let runs = match hx::find_ci(text, q.trim()) {
        Some((a, b)) => [
            run(a, color, None),
            run(b - a, ACCENT_TEXT, Some(ACCENT_SOFT)),
            run(text.len() - b, color, None),
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        None => vec![run(text.len(), color, None)],
    };
    StyledText::new(text.to_string()).with_runs(runs)
}

/// Up to four snippets: the first request, the first match of `q`, the last two.
fn pick_snippets<'a>(c: &'a ChatMeta, q: &str) -> Vec<(&'a Snippet, bool)> {
    let q = q.trim().to_lowercase();
    let n = c.snippets.len();
    let mut ix: Vec<usize> = Vec::new();
    if let Some(i) = c.snippets.iter().position(|s| s.who == "You") {
        ix.push(i);
    }
    let hit = (!q.is_empty())
        .then(|| {
            c.snippets
                .iter()
                .position(|s| s.text.to_lowercase().contains(&q))
        })
        .flatten();
    if let Some(i) = hit {
        ix.push(i);
    }
    ix.extend(n.saturating_sub(2)..n);
    ix.sort_unstable();
    ix.dedup();
    if ix.len() > 4 {
        // Keep the first, the match, and the last ones.
        let keep_hit = hit.filter(|h| *h != ix[0]);
        let mut trimmed = vec![ix[0]];
        trimmed.extend(keep_hit);
        trimmed.extend(ix.iter().rev().take(4 - trimmed.len()).copied());
        trimmed.sort_unstable();
        trimmed.dedup();
        ix = trimmed;
    }
    ix.into_iter()
        .map(|i| {
            let s = &c.snippets[i];
            (s, !q.is_empty() && s.text.to_lowercase().contains(&q))
        })
        .collect()
}

/// Ellipsis line (absolute box so the cell's width decides where it cuts).
fn ellipsis(el: impl IntoElement, h: f32) -> gpui::Div {
    div().relative().flex_1().min_w_0().h(px(h)).child(
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(el),
    )
}

fn checkbox(checked: bool) -> gpui::Div {
    div()
        .size(px(16.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(if checked { ACCENT_STRONG } else { LINE_STRONG })
        .bg(if checked { ACCENT_STRONG } else { SURFACE_RAISED })
        .when(checked, |d| d.child(icon("check", 12., ON_ACCENT)))
}

fn hint(keys: &[&'static str], label: &'static str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .children(keys.iter().map(|k| kbd(*k)))
        .child(div().ml(px(4.)).child(label))
}

// ── Render ──────────────────────────────────────────────────────────────

impl ChatHistory {
    fn render_search(&self, cx: &mut Context<Self>) -> gpui::Div {
        let filtering = self.filter.is_filtering();
        let total = self
            .chats
            .iter()
            .filter(|c| !self.flags.is_archived(&c.id))
            .count();
        let count = if self.loading && self.chats.is_empty() {
            "Loading…".to_string()
        } else if filtering {
            let shown = hx::filter(&self.chats, &self.flags, &self.filter, &|c| {
                self.status_of(&c.id)
            })
            .len();
            format!("{shown} match · {total} chats")
        } else {
            format!("{total} chats")
        };
        let tokens = self.filter.tokens.iter().enumerate().map(|(i, t)| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(px(26.))
                .pl(px(8.))
                .pr(px(4.))
                .rounded(px(RADIUS_MD))
                .bg(ACCENT_SOFT)
                .border_1()
                .border_color(ACCENT_RING)
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .text_color(ACCENT_TEXT)
                .child(t.clone())
                .child(
                    div()
                        .id(eid("hs-token-x", i))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(RADIUS_SM))
                        .cursor_pointer()
                        .hover(|s| s.bg(ACCENT_GLOW))
                        .child(icon("x", 12., ACCENT_TEXT))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if i < this.filter.tokens.len() {
                                this.filter.tokens.remove(i);
                            }
                            cx.notify();
                        })),
                )
        });
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .min_h(px(52.))
            .px(px(14.))
            .py(px(8.))
            .border_b_1()
            .border_color(LINE)
            .child(icon("search", 16., TEXT_MUTED))
            .children(tokens)
            .child(
                div()
                    .flex_1()
                    .min_w(px(280.))
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .px(px(6.))
                    .text_size(px(14.))
                    .child(div().flex_1().h(px(20.)).child(self.query.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .whitespace_nowrap()
                    .child(count),
            )
            .child(
                div()
                    .id("hs-esc")
                    .p(px(2.))
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .child(kbd("Esc"))
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> gpui::Div {
        let total = self
            .chats
            .iter()
            .filter(|c| !self.flags.is_archived(&c.id))
            .count();
        let scopes = [
            (Scope::All, "All", total),
            (Scope::Mine, "Mine", total),
            (Scope::Agents, "Started by agents", 0),
        ];
        let seg = div()
            .flex()
            .flex_none()
            .p(px(2.))
            .rounded(px(RADIUS_LG))
            .bg(BG_SIDEBAR)
            .border_1()
            .border_color(LINE)
            .children(scopes.into_iter().enumerate().map(|(i, (scope, label, n))| {
                let on = self.filter.scope == scope;
                div()
                    .id(eid("hs-scope", i))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(RADIUS_MD))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(on, |d| d.bg(SURFACE_HOVER).text_color(TEXT))
                    .when(!on, |d| d.text_color(TEXT_SECONDARY).hover(|s| s.text_color(TEXT)))
                    .child(label)
                    .child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(TEXT_MUTED)
                            .child(n.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter.scope = scope;
                        cx.notify();
                    }))
            }));
        let chip = |id: &'static str, label: &'static str, dot: Option<Status>, on: bool| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(RADIUS_MD))
                .border_1()
                .text_size(px(12.))
                .cursor_pointer()
                .when(on, |d| {
                    d.bg(ACCENT_SOFT)
                        .border_color(ACCENT_RING)
                        .text_color(ACCENT_TEXT)
                })
                .when(!on, |d| {
                    d.border_color(LINE_STRONG)
                        .text_color(TEXT_SECONDARY)
                        .hover(|s| s.bg(SURFACE_RAISED))
                })
                .when_some(dot, |d, st| d.child(status_dot(st, true)))
                .child(label)
        };
        let f = &self.filter;
        let sort_label = match f.sort {
            Sort::Recent => "Last activity",
            Sort::Cost => "Cost",
        };
        let sort = div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id("hs-sort")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(SURFACE_RAISED)
                    .text_size(px(12.))
                    .text_color(TEXT_SECONDARY)
                    .cursor_pointer()
                    .child(sort_label)
                    .child(icon("chevron-down", 12., TEXT_MUTED))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sort_open = !this.sort_open;
                        cx.notify();
                    })),
            )
            .when(self.sort_open, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(32.))
                        .right_0()
                        .w(px(160.))
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .rounded(px(RADIUS_LG))
                        .border_1()
                        .border_color(LINE_STRONG)
                        .bg(SURFACE_RAISED)
                        .shadow(popover_shadow())
                        .occlude()
                        .children([(Sort::Recent, "Last activity"), (Sort::Cost, "Cost")].into_iter().enumerate().map(
                            |(i, (s, label))| {
                                let on = self.filter.sort == s;
                                div()
                                    .id(eid("hs-sort-opt", i))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .h(px(28.))
                                    .px(px(8.))
                                    .rounded(px(RADIUS_MD))
                                    .text_size(px(12.))
                                    .text_color(if on { TEXT } else { TEXT_SECONDARY })
                                    .cursor_pointer()
                                    .hover(|st| st.bg(SURFACE_HOVER))
                                    .child(label)
                                    .when(on, |d| d.child(icon("check", 12., ACCENT_TEXT)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.filter.sort = s;
                                        this.sort_open = false;
                                        cx.notify();
                                    }))
                            },
                        )),
                )
            });
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(8.))
            .border_b_1()
            .border_color(LINE)
            .child(seg)
            .child(div().w(px(1.)).h(px(20.)).bg(LINE))
            .child(
                chip("hs-q-waiting", "Waiting for you", Some(Status::Waiting), f.waiting).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.filter.waiting = !this.filter.waiting;
                        cx.notify();
                    }),
                ),
            )
            .child(
                chip("hs-q-running", "Running", Some(Status::Running), f.running).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.filter.running = !this.filter.running;
                        cx.notify();
                    }),
                ),
            )
            .child(
                chip("hs-q-failed", "Failed", Some(Status::Error), f.failed).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.filter.failed = !this.filter.failed;
                        cx.notify();
                    },
                )),
            )
            .child(
                chip("hs-q-files", "Edited files", None, f.has_files).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.filter.has_files = !this.filter.has_files;
                        cx.notify();
                    },
                )),
            )
            .child(div().flex_1())
            .child(div().text_size(px(12.)).text_color(TEXT_MUTED).child("Sort"))
            .child(sort)
            .child(
                Button::new("hs-select", if self.selecting { "Done" } else { "Select" })
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selecting = !this.selecting;
                        this.checked.clear();
                        this.confirm_delete = false;
                        cx.notify();
                    })),
            )
    }

    fn render_bulk(&self, cx: &mut Context<Self>) -> gpui::Div {
        let n = self.checked.len();
        let none = n == 0;
        let bar = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(8.))
            .border_b_1()
            .border_color(LINE)
            .bg(ACCENT_GLOW);
        if self.confirm_delete {
            return bar
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(DANGER)
                        .child(format!(
                            "Delete {n} chat{} permanently? This cannot be undone.",
                            if n == 1 { "" } else { "s" }
                        )),
                )
                .child(div().flex_1())
                .child(
                    Button::new("hs-del-cancel", "Cancel")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_delete = false;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("hs-del-yes", "Delete")
                        .small()
                        .variant(ui::ButtonVariant::Danger)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_checked(cx))),
                );
        }
        bar.child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .child(format!("{n} selected")),
        )
        .child(div().flex_1())
        .child(
            Button::new("hs-bulk-archive", "Archive")
                .small()
                .disabled(none)
                .on_click(cx.listener(|this, _, _, cx| {
                    let ids: Vec<String> = this.checked.iter().cloned().collect();
                    this.archive(ids, cx);
                })),
        )
        .child(
            Button::new("hs-bulk-export", "Export .md")
                .small()
                .disabled(none)
                .on_click(cx.listener(|this, _, _, cx| this.export_checked(cx))),
        )
        .child(
            Button::new("hs-bulk-delete", "Delete…")
                .small()
                .variant(ui::ButtonVariant::Danger)
                .disabled(none)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_delete = true;
                    cx.notify();
                })),
        )
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.groups();
        let sel = self.selected().map(|c| c.id.clone());
        let now = hx::now_secs();
        let offset = hx::utc_offset(now);
        let q = self.filter.query.clone();
        let mut children: Vec<AnyElement> = Vec::new();
        let mut row_ix = 0;
        for (gi, g) in groups.iter().enumerate() {
            let key = g.key;
            let open = g.open;
            children.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(10.))
                    .px(px(8.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(
                        div()
                            .id(eid("hs-group", gi))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_color(TEXT_SECONDARY)
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .child(icon(
                                if open { "chevron-down" } else { "chevron-right" },
                                12.,
                                TEXT_MUTED,
                            ))
                            .child(key.name())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.group_open.insert(key, !open);
                                cx.notify();
                            })),
                    )
                    .child(g.count.clone())
                    .child(div().flex_1().h(px(1.)).bg(LINE_SUBTLE))
                    .when(g.cost > 0., |d| d.child(format!("${:.2}", g.cost)))
                    .into_any_element(),
            );
            for c in &g.rows {
                let on = sel.as_deref() == Some(c.id.as_str());
                children.push(self.render_row(row_ix, c, on, &q, now, offset, cx));
                row_ix += 1;
            }
            if g.more > 0 {
                let more = g.more;
                children.push(
                    div()
                        .id(eid("hs-more", gi))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .h(px(32.))
                        .mt(px(2.))
                        .mb(px(4.))
                        .rounded(px(RADIUS_LG))
                        .border_1()
                        .border_dashed()
                        .border_color(LINE_STRONG)
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .cursor_pointer()
                        .hover(|s| s.bg(SURFACE_RAISED))
                        .child(format!(
                            "Load {} more · {more} in {}",
                            more.min(PAGE),
                            key.name().to_lowercase()
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            *this.limits.entry(key).or_insert(PAGE) += PAGE;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let empty = children.is_empty();
        div()
            .id("hs-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(6.))
            .pb(px(8.))
            .border_r_1()
            .border_color(LINE)
            .when(empty, |d| {
                d.child(
                    div()
                        .py(px(32.))
                        .px(px(16.))
                        .flex()
                        .justify_center()
                        .text_color(TEXT_MUTED)
                        .child(if self.loading {
                            "Loading chats…"
                        } else if self.chats.is_empty() {
                            "No chats yet. Start one with New chat."
                        } else {
                            "No chats match. Try fewer filters."
                        }),
                )
            })
            .children(children)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        ix: usize,
        c: &ChatMeta,
        on: bool,
        q: &str,
        now: u64,
        offset: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (st, st_text, st_color) = live_view(self.status_of(&c.id));
        let pinned = self.flags.is_pinned(&c.id);
        let checked = self.checked.contains(&c.id);
        let id = c.id.clone();
        let id2 = c.id.clone();
        let tile_icon = match c.agent.as_str() {
            "Plan" => "policy",
            _ => "agent",
        };
        div()
            .id(eid("hs-row", ix))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .min_h(px(52.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(RADIUS_LG))
            .cursor_pointer()
            .when(on, |d| d.bg(SURFACE_HOVER))
            .when(!on, |d| d.hover(|s| s.bg(SURFACE_RAISED)))
            .when(on, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .rounded_l(px(RADIUS_LG))
                        .bg(ACCENT),
                )
            })
            .when(self.selecting, |d| {
                d.child(
                    div()
                        .id(eid("hs-check", ix))
                        .child(checkbox(checked))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.checked.remove(&id2) {
                                this.checked.insert(id2.clone());
                            }
                            this.confirm_delete = false;
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .size(px(26.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_MD))
                    .bg(SURFACE_HOVER)
                    .child(icon(tile_icon, 14., TEXT_SECONDARY)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .when(pinned, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(ACCENT_TEXT)
                                        .child("●"),
                                )
                            })
                            .child(ellipsis(
                                highlighted(&c.title, q, FontWeight::MEDIUM, TEXT),
                                LINE_BODY,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .child(div().flex_none().child(c.agent.clone()))
                            .when(!c.wd_label.is_empty(), |d| {
                                d.child(div().flex_none().text_color(TEXT_DISABLED).child("·"))
                                    .child(
                                        ellipsis(c.wd_label.clone(), 16.)
                                            .font_family(FONT_MONO)
                                            .text_size(px(11.)),
                                    )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .items_end()
                    .gap(px(2.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_color(st_color)
                            .child(status_dot(st, true))
                            .child(st_text),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(TEXT_MUTED)
                            .child(format!(
                                "{} · ${:.2}",
                                hx::when_label(c.updated, now, offset),
                                c.cost_usd
                            )),
                    ),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    this.open_chat(id.clone(), cx);
                } else {
                    this.sel = Some(id.clone());
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> gpui::Div {
        let aside = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(400.))
            .h_full()
            .bg(BG_SIDEBAR);
        let Some(c) = self.selected() else {
            return aside;
        };
        let (_, st_text, st_color) = live_view(self.status_of(&c.id));
        let q = self.filter.query.clone();
        let pinned = self.flags.is_pinned(&c.id);
        let wd = [self.host.as_str(), c.wd_label.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let snippets = pick_snippets(c, &q).into_iter().map(|(s, hit)| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(TEXT_MUTED)
                        .child(s.who.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(TEXT_SECONDARY)
                        .rounded(px(RADIUS_LG))
                        .when(hit, |d| d.bg(ACCENT_GLOW).px(px(8.)).py(px(6.)))
                        .child(highlighted(&s.text, &q, FontWeight::NORMAL, TEXT_SECONDARY)),
                )
        });
        let files: Vec<String> = if c.files.is_empty() {
            vec!["no files changed".into()]
        } else {
            c.files.clone()
        };
        aside
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(6.))
                    .px(px(18.))
                    .py(px(16.))
                    .border_b_1()
                    .border_color(LINE)
                    .child(
                        div()
                            .text_size(px(16.))
                            .line_height(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(c.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_x(px(10.))
                            .gap_y(px(4.))
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child(div().text_color(st_color).child(st_text))
                            .child(c.agent.clone())
                            .child(format!(
                                "{} message{}",
                                c.messages,
                                if c.messages == 1 { "" } else { "s" }
                            ))
                            .child(format!("${:.2}", c.cost_usd)),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .child(wd),
                    ),
            )
            .child(
                div()
                    .id("hs-preview")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap(px(10.))
                    .px(px(18.))
                    .py(px(14.))
                    .children(snippets)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .gap(px(4.))
                            .mt(px(4.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(TEXT_MUTED)
                                    .child("Files touched"),
                            )
                            .children(files.into_iter().map(|f| {
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(TEXT_SECONDARY)
                                    .child(f)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_wrap()
                    .gap(px(8.))
                    .px(px(18.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(LINE)
                    .child(
                        div()
                            .id("hs-open")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(32.))
                            .px(px(12.))
                            .rounded(px(RADIUS_LG))
                            .bg(ACCENT_STRONG)
                            .text_color(ON_ACCENT)
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .hover(|s| s.bg(ACCENT_STRONG_HOVER))
                            .child("Open")
                            .child(kbd("↵").text_color(ON_ACCENT).border_color(ACCENT_RING))
                            .on_click(cx.listener(|this, _, _, cx| this.open_selected(cx))),
                    )
                    .child(
                        Button::new("hs-fork", "Fork")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.fork_selected(cx))),
                    )
                    .child(
                        Button::new("hs-pin", if pinned { "Unpin" } else { "Pin" })
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_pin(&TogglePinSelected, window, cx)
                            })),
                    )
                    .child(
                        Button::new("hs-archive", "Archive")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.archive_selected(&ArchiveSelected, window, cx)
                            })),
                    ),
            )
    }

    fn render_footer(&self) -> gpui::Div {
        let total = self
            .chats
            .iter()
            .filter(|c| !self.flags.is_archived(&c.id))
            .count();
        let messages: usize = self.chats.iter().map(|c| c.messages).sum();
        let messages = if messages >= 10_000 {
            format!("{}k", messages / 1000)
        } else {
            messages.to_string()
        };
        let age = hx::now_secs().saturating_sub(self.loaded_at);
        let updated = if self.loading {
            "updating…".to_string()
        } else if age < 60 {
            "updated just now".to_string()
        } else {
            format!("updated {} min ago", age / 60)
        };
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_x(px(14.))
            .gap_y(px(6.))
            .px(px(14.))
            .py(px(8.))
            .border_t_1()
            .border_color(LINE)
            .text_size(px(12.))
            .text_color(TEXT_MUTED)
            .child(hint(&["↑", "↓"], "move"))
            .child(hint(&["↵"], "open"))
            .child(hint(&["⌘P"], "pin"))
            .child(hint(&["⌘Bksp"], "archive"))
            .child(div().flex_1())
            .child(format!("Index: {total} chats · {messages} messages · {updated}"))
    }

    fn render_toast(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let t = self.toast.as_ref()?;
        let can_undo = !t.undo.is_empty();
        Some(
            div()
                .absolute()
                .bottom(px(52.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .py(px(8.))
                        .pl(px(14.))
                        .pr(px(8.))
                        .rounded(px(RADIUS_LG))
                        .border_1()
                        .border_color(LINE_STRONG)
                        .bg(SURFACE_RAISED)
                        .shadow(popover_shadow())
                        .child(t.text.clone())
                        .when(can_undo, |d| {
                            d.child(
                                Button::new("hs-undo", "Undo")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                            )
                        }),
                ),
        )
    }
}

impl Focusable for ChatHistory {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ChatHistory {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let dialog = div()
            .id("chat-history")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::remove_last_token))
            .on_action(cx.listener(Self::archive_selected))
            .on_action(cx.listener(Self::toggle_pin))
            .on_key_down(cx.listener(Self::on_key))
            .relative()
            .flex()
            .flex_col()
            .w(px(1080.))
            .max_w(relative(0.94))
            .h(px(760.))
            .max_h(relative(0.86))
            .rounded(px(12.))
            .border_1()
            .border_color(LINE_STRONG)
            .bg(SURFACE)
            .overflow_hidden()
            .shadow(popover_shadow())
            .occlude()
            // Clicks inside the dialog must not reach the backdrop.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.render_search(cx))
            .child(self.render_filters(cx))
            .when(self.selecting, |d| d.child(self.render_bulk(cx)))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_list(cx))
                    .child(self.render_preview(cx)),
            )
            .child(self.render_footer())
            .children(self.render_toast(cx));
        div()
            .id("chat-history-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(96.))
            .bg(gpui::hsla(210. / 360., 0.11, 0.035, 0.72))
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(dialog)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(snips: &[(&str, &str)]) -> ChatMeta {
        let messages: Vec<serde_json::Value> = snips
            .iter()
            .map(|(who, text)| {
                serde_json::json!({
                    "role": if *who == "You" { "user" } else { "assistant" },
                    "content": [{"Text": text}],
                })
            })
            .collect();
        hx::meta_from_json(&serde_json::json!({"id": "x", "messages": messages}), 0).unwrap()
    }

    #[test]
    fn snippets_first_match_and_last_two() {
        let c = meta(&[
            ("You", "start"),
            ("Build", "one"),
            ("Build", "needle here"),
            ("Build", "three"),
            ("Build", "four"),
            ("Build", "five"),
        ]);
        let picked: Vec<&str> = pick_snippets(&c, "needle")
            .iter()
            .map(|(s, _)| s.text.as_str())
            .collect();
        assert_eq!(picked, vec!["start", "needle here", "four", "five"]);
        let plain: Vec<&str> = pick_snippets(&c, "")
            .iter()
            .map(|(s, _)| s.text.as_str())
            .collect();
        assert_eq!(plain, vec!["start", "four", "five"]);
    }

    #[test]
    fn group_order() {
        let mut g = vec![
            Group::Month(2026, 9),
            Group::Today,
            Group::Pinned,
            Group::Month(2026, 10),
            Group::Yesterday,
        ];
        g.sort_by_key(|g| group_rank(*g));
        assert_eq!(
            g,
            vec![
                Group::Pinned,
                Group::Today,
                Group::Yesterday,
                Group::Month(2026, 10),
                Group::Month(2026, 9)
            ]
        );
    }
}
