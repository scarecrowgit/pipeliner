//! Task thread: one task's conversation (agent messages, tool calls, your steering),
//! its sub-tasks and controls (steer, cancel, retry). Design: design/screens/task-thread.html.
//! Starting point: header and the message list.

use harness_core::{Message, Role, Task as CoreTask};

use super::prelude::*;

struct ThreadData {
    task: Option<CoreTask>,
    messages: Vec<Message>,
}

pub struct TaskScreen {
    task_id: Option<Id>,
    data: Option<ThreadData>,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl TaskScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
            AppEvent::Navigated(Route::Task { task }) => {
                if this.task_id.as_ref() != Some(task) {
                    this.task_id = Some(task.clone());
                    this.data = None;
                    this.reload(cx);
                }
            }
            AppEvent::StoreChanged { changes, .. } => {
                let Some(id) = this.task_id.clone() else { return };
                let relevant = changes.iter().any(|c| match c {
                    Change::Task { id: t, .. } => *t == id,
                    Change::Message { thread_id, .. } => *thread_id == id,
                    Change::External => true,
                    _ => false,
                });
                if relevant {
                    this.reload(cx);
                }
            }
            _ => {}
        });
        let task_id = match state::route(cx) {
            Route::Task { task } => Some(task),
            _ => None,
        };
        let mut this = TaskScreen { task_id, data: None, load: None, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.task_id.clone() else { return };
        self.load = Some(load(
            cx,
            move |s| ThreadData { task: s.task(&id), messages: s.messages(&id) },
            |this, d, _| this.data = Some(d),
        ));
    }
}

impl Render for TaskScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let back = IconButton::new("task-back", AppIcon::ArrowLeft).tooltip("Back").on_click(|_, _, cx| state::back(cx));
        let (header, content) = match &self.data {
            None => (PageHeader::new("Task").meta(back), body().child(skeleton_rows("task-loading", 5))),
            Some(ThreadData { task: None, .. }) => (
                PageHeader::new("Task").meta(back),
                body().child(EmptyState::new(AppIcon::Alert, "Task not found", "It may have been removed from the store.")),
            ),
            Some(ThreadData { task: Some(t), messages }) => {
                let header = PageHeader::new(t.title.clone())
                    .meta(back)
                    .meta(StatusPill::new(t.status).id("task-status"))
                    .when(!t.model.is_empty(), |h| h.meta(Badge::new(Config::short_model(&t.model))))
                    .meta(div().text_mono().fg_muted().child(ui::fmt_usd(t.cost_usd)));
                let content = if messages.is_empty() {
                    body().child(
                        EmptyState::new(AppIcon::Chat, "No messages yet", "The agent's messages, tool calls and your notes appear here.")
                            .id("task-empty"),
                    )
                } else {
                    body().gap(theme::S4).children(messages.iter().rev().take(200).rev().map(|m| {
                        let who = match m.role {
                            Role::User => "You".to_string(),
                            Role::Assistant => if m.author.is_empty() { t.actor.clone() } else { m.author.clone() },
                            Role::Tool => "Tool".to_string(),
                            Role::System => "Pipeliner".to_string(),
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme::S1)
                            .max_w(px(820.))
                            .child(
                                div()
                                    .flex()
                                    .gap(theme::S2)
                                    .text_label()
                                    .child(div().fg_secondary().font_weight(gpui::FontWeight::MEDIUM).child(who))
                                    .child(div().fg_muted().child(m.created_at.with_timezone(&chrono::Local).format("%H:%M").to_string())),
                            )
                            .child(match m.role {
                                Role::Tool => CodeBlock::new(SharedString::from(format!("tool-{}", m.id)), m.content.clone())
                                    .max_height(px(240.))
                                    .into_any_element(),
                                _ => ui::markdown(SharedString::from(format!("msg-{}", m.id)), m.content.clone(), window, cx)
                                    .into_any_element(),
                            })
                    }))
                };
                (header, content)
            }
        };
        screen().child(header).child(content)
    }
}
