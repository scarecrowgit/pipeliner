//! Home dashboard: totals, what is working now, what needs the user, and spend.
//!
//! Port of `reference/home_dashboard.html`. Everything is the mockup's mock data except
//! chat sessions: running / waiting chats published by the Chat screen (`ui::nav`) replace
//! the mock chat rows in "Working now" and "Needs you".

mod charts;
mod data;
mod rows;
mod tooltip;
mod view;

use std::time::Duration;

use gpui::{
    App, Context, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use ui::nav::{self, ChatSessionInfo};
use ui::theme::*;

use data::{Range, Target};
use rows::{LiveRow, NeedRow};

/// How often relative ages ("12m") of real chats are refreshed.
const AGE_REFRESH: Duration = Duration::from_secs(30);

pub struct HomeScreen {
    range: Range,
    /// Running / waiting chat sessions from the nav entity.
    chats: Vec<ChatSessionInfo>,
    /// Runtime shown for real chats (they run on this machine).
    host: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl HomeScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let mut chats = Vec::new();
        if let Some(nav) = nav::nav_entity(cx) {
            chats = rows::active_chats(&nav.read(cx).chat_sessions);
            subscriptions.push(cx.observe(&nav, |this: &mut Self, nav, cx| {
                let chats = rows::active_chats(&nav.read(cx).chat_sessions);
                if chats != this.chats {
                    this.chats = chats;
                    cx.notify();
                }
            }));
        }
        // Keep the ages of real chats current.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGE_REFRESH).await;
                let alive = this.update(cx, |this, cx| {
                    if !this.chats.is_empty() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();

        Self {
            range: Range::default(),
            chats,
            host: rows::hostname(),
            _subscriptions: subscriptions,
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
        Target::Agent(name) => nav::select_agent(*name, true, cx),
        Target::Screen(screen) => nav::navigate(*screen, cx),
        Target::Chat(id) => nav::request_chat(nav::ChatRequest::Open(id.clone()), cx),
    }
}

impl Render for HomeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = rows::now();
        let live: Vec<LiveRow> = rows::live_rows(&self.chats, &self.host, now);
        let needs: Vec<NeedRow> = rows::need_rows(&self.chats, now);

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.render_header(cx))
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
                            .child(self.render_kpis(live.len(), needs.len()))
                            .child(
                                wrap_row()
                                    .child(view::working_now(&live))
                                    .child(view::needs_you(&needs)),
                            )
                            .child(
                                wrap_row()
                                    .child(charts::spend_per_day())
                                    .child(charts::spend_by_agent(self.range))
                                    .child(charts::runtimes()),
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
