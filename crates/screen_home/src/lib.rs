//! Home dashboard: totals, what is working now, what needs the user, and spend.
//!
//! Layout of `reference/home_dashboard.html`; every number comes from real data: the
//! pipeline service (issues and their latest runs), chat sessions published by the Chat
//! screen (`ui::nav`) and the Inbox file. A fresh install shows empty states.

mod charts;
mod data;
mod rows;
mod stats;
mod tooltip;
mod view;

#[cfg(test)]
mod stats_tests;

use std::time::Duration;

use agent_tools::inbox::{self, InboxItem};
use gpui::{
    App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use pipeline::Pipelines;
use ui::nav::{self, ChatSessionInfo};
use ui::theme::*;

use data::{Range, Target};
use stats::RunSummary;

/// How often ages, elapsed times and the day buckets are refreshed.
const REFRESH: Duration = Duration::from_secs(30);

pub struct HomeScreen {
    range: Range,
    /// Latest run of every issue that has one (newest issue first).
    runs: Vec<RunSummary>,
    /// Issues on the board.
    issue_count: usize,
    workspace: SharedString,
    /// Chat sessions from the nav entity (current, pinned and recent).
    chats: Vec<ChatSessionInfo>,
    /// Inbox reports; reloaded when `nav.inbox_version` changes.
    inbox: Vec<InboxItem>,
    inbox_version: Option<u64>,
    /// This machine: the runtime of pipeline workers and chats.
    host: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl HomeScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            range: Range::default(),
            runs: Vec::new(),
            issue_count: 0,
            workspace: SharedString::default(),
            chats: Vec::new(),
            inbox: Vec::new(),
            inbox_version: None,
            host: rows::hostname(),
            _subscriptions: Vec::new(),
        };

        if let Some(nav) = nav::nav_entity(cx) {
            this.sync_nav(&nav, cx);
            this._subscriptions
                .push(cx.observe(&nav, |this: &mut Self, nav, cx| this.sync_nav(&nav, cx)));
        } else {
            this.reload_inbox(cx);
        }
        if let Some(pipelines) = pipeline::global(cx) {
            this.sync_pipelines(&pipelines, cx);
            this._subscriptions
                .push(cx.observe(&pipelines, |this: &mut Self, p, cx| {
                    this.sync_pipelines(&p, cx)
                }));
        }

        // Keep ages, elapsed times and "today" current.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        })
        .detach();

        this
    }

    /// Chat sessions changed, or the Inbox did (reload it).
    fn sync_nav(&mut self, nav: &Entity<nav::Nav>, cx: &mut Context<Self>) {
        let n = nav.read(cx);
        let (chats, version) = (n.chat_sessions.clone(), n.inbox_version);
        if chats != self.chats {
            self.chats = chats;
            cx.notify();
        }
        if self.inbox_version != Some(version) {
            self.inbox_version = Some(version);
            self.reload_inbox(cx);
        }
    }

    fn reload_inbox(&mut self, cx: &mut Context<Self>) {
        let load = cx.background_spawn(async { inbox::load() });
        cx.spawn(async move |this, cx| {
            let items = load.await;
            this.update(cx, |this, cx| {
                if this.inbox != items {
                    this.inbox = items;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Copy what Home shows of each issue's latest run (without its log).
    fn sync_pipelines(&mut self, pipelines: &Entity<Pipelines>, cx: &mut Context<Self>) {
        let p = pipelines.read(cx);
        let runs: Vec<RunSummary> = p
            .issues
            .iter()
            .filter_map(|issue| {
                let active = p.is_active(&issue.key);
                p.with_run(&issue.key, |run| RunSummary::new(run, &issue.title, active))
            })
            .collect();
        let issue_count = p.issues.len();
        let workspace: SharedString = p.workspace_name().into();
        if runs != self.runs || issue_count != self.issue_count || workspace != self.workspace {
            self.runs = runs;
            self.issue_count = issue_count;
            self.workspace = workspace;
            cx.notify();
        }
    }

    fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if self.range != range {
            self.range = range;
            cx.notify();
        }
    }
}

/// Follow a row's link.
fn open(target: &Target, cx: &mut App) {
    match target {
        Target::Issue(key) => nav::open_issue(key.clone(), cx),
        Target::Chat(id) => nav::request_chat(nav::ChatRequest::Open(id.clone()), cx),
        Target::Inbox(id) => nav::open_inbox_item(id.clone(), cx),
    }
}

impl Render for HomeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = ui::time::now_secs();
        let offset = ui::time::utc_offset(now);
        let start = self.range.start(now, offset);
        let kpis = stats::kpis(&self.runs, &self.chats, &self.inbox, start);
        let live = rows::live_rows(&self.runs, &self.chats, &self.host, now);
        let needs = rows::need_rows(&self.runs, &self.chats, &self.inbox, now);
        let days = stats::daily_spend(&self.runs, &self.chats, now, offset);
        let by_agent = stats::spend_by_agent(&self.runs, &self.chats, start);

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.render_header(now, offset, cx))
            .child(
                div()
                    .id("home-main")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(SPACE_4))
                            .pt(px(SPACE_5))
                            .px(px(SPACE_6))
                            .pb(px(SPACE_6))
                            .child(self.render_kpis(&kpis, now))
                            .child(
                                wrap_row()
                                    .child(view::working_now(&live, self.issue_count))
                                    .child(view::needs_you(&needs)),
                            )
                            .child(
                                wrap_row()
                                    .child(charts::spend_per_day(&days, offset))
                                    .child(charts::spend_by_agent(&by_agent, self.range))
                                    .child(charts::runtimes(&self.host, &kpis)),
                            ),
                    ),
            )
    }
}

/// Row of cards that wraps when the window is narrow (`display: flex; flex-wrap: wrap`).
fn wrap_row() -> gpui::Div {
    div().flex().flex_none().flex_wrap().gap(px(SPACE_4))
}

/// `flex: <grow> 1 <basis>px; min-width: 0`.
pub(crate) fn flex_item(grow: f32, basis: f32) -> gpui::Div {
    let mut d = div().flex_basis(px(basis)).flex_shrink().min_w_0();
    d.style().flex_grow = Some(grow);
    d
}
