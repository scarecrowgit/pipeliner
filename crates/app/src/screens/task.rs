//! Task thread: one task's conversation (agent messages, tool calls, your steering), its
//! lineage (parent and spawned sub-tasks) and controls (steer, cancel, retry).
//! Design: design/screens/task-thread.html.
//!
//! * The store is queried off the UI thread into [`ThreadData`]: messages are already folded
//!   into display [`Entry`]s (tool calls paired with their results), so `render` stays cheap.
//! * The thread sticks to the bottom while it grows; scrolling up releases it and shows
//!   "Jump to latest".
//! * A chat task's conversation lives in its chat (thread id = `chat_id`); everything else
//!   uses the task id as thread id.

use std::collections::HashSet;
use std::time::Duration;

use chrono::Utc;
use gpui::{
    pulsating_between, Animation, AnimationExt as _, ClipboardItem, ScrollHandle, ScrollWheelEvent,
};
use gpui_component::input::InputState;
use harness_core::{Message, Role, Task as CoreTask, TaskKind};
use serde_json::Value;

use super::prelude::*;
use crate::ui::input::{self, InputEvent};

/// Entries rendered before "Show earlier messages".
const PAGE: usize = 150;

/// One row of the thread, ready to render.
enum Entry {
    User {
        id: SharedString,
        text: SharedString,
        author: SharedString,
        time: SharedString,
        steer: bool,
        delivered: bool,
    },
    Assistant {
        id: SharedString,
        text: SharedString,
        author: SharedString,
        time: SharedString,
        partial: bool,
    },
    /// A tool call and its result (if it already ran).
    Tool {
        id: SharedString,
        name: SharedString,
        summary: SharedString,
        args: SharedString,
        result: Option<SharedString>,
        failed: bool,
    },
    System {
        id: SharedString,
        text: SharedString,
        time: SharedString,
    },
}

/// A task shown in the lineage panel.
struct Rel {
    id: Id,
    title: SharedString,
    actor: SharedString,
    status: Status,
    cost: f64,
}

/// A breadcrumb segment; `route` makes it a link.
struct Crumb {
    label: SharedString,
    route: Option<Route>,
}

struct ThreadData {
    task: Option<CoreTask>,
    thread_id: Id,
    entries: Vec<Entry>,
    crumbs: Vec<Crumb>,
    parent: Option<Rel>,
    children: Vec<Rel>,
}

pub struct TaskScreen {
    task_id: Option<Id>,
    data: Option<ThreadData>,
    /// Which tool rows are expanded (by entry id).
    expanded: HashSet<SharedString>,
    shown: usize,
    stick: bool,
    scroll: ScrollHandle,
    ticking: bool,
    composer: Entity<InputState>,
    loading: bool,
    pending: bool,
    load: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl TaskScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, _, cx| match ev {
            AppEvent::Navigated(Route::Task { task }) => {
                if this.task_id.as_ref() != Some(task) {
                    this.open(task.clone(), cx);
                }
            }
            AppEvent::StoreChanged { changes, .. } => {
                let Some(id) = this.task_id.clone() else {
                    return;
                };
                let (thread, run) = this
                    .data
                    .as_ref()
                    .map(|d| {
                        (
                            d.thread_id.clone(),
                            d.task.as_ref().and_then(|t| t.run_id.clone()),
                        )
                    })
                    .unwrap_or((id.clone(), None));
                let relevant = changes.iter().any(|c| match c {
                    Change::Task { id: t, run_id } => {
                        *t == id || run_id.is_none() || (run.is_some() && *run_id == run)
                    }
                    Change::Message { thread_id, .. } => *thread_id == thread || *thread_id == id,
                    Change::Run(_) | Change::Pipeline(_) | Change::Chat(_) | Change::External => {
                        true
                    }
                    _ => false,
                });
                if relevant {
                    this.reload(cx);
                }
            }
            _ => {}
        });

        let composer = input::text_input(
            "Steer the agent, answer a question, or ask why it chose this…",
            window,
            cx,
        );
        let enter_sub =
            cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.send(window, cx);
                }
            });

        let task_id = match state::route(cx) {
            Route::Task { task } => Some(task),
            _ => None,
        };
        let mut this = TaskScreen {
            task_id,
            data: None,
            expanded: HashSet::new(),
            shown: PAGE,
            stick: true,
            scroll: ScrollHandle::new(),
            ticking: false,
            composer,
            loading: false,
            pending: false,
            load: None,
            _subs: vec![sub, enter_sub],
        };
        this.reload(cx);
        this
    }

    /// Switches to another task: forget the old thread's view state.
    fn open(&mut self, id: Id, cx: &mut Context<Self>) {
        self.task_id = Some(id);
        self.data = None;
        self.expanded.clear();
        self.shown = PAGE;
        self.stick = true;
        self.reload(cx);
        cx.notify();
    }

    /// Re-queries the thread. Overlapping requests are coalesced: while one is in flight,
    /// further changes just mark the result stale and trigger one more query afterwards.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.task_id.clone() else {
            return;
        };
        if self.loading {
            self.pending = true;
            return;
        }
        self.loading = true;
        let wanted = id.clone();
        self.load = Some(load(
            cx,
            move |s| query(s, &id),
            move |this, d, cx| {
                this.loading = false;
                // Ignore a result for a task we already navigated away from.
                if this.task_id.as_ref() == Some(&wanted) {
                    this.data = Some(d);
                    this.ensure_ticker(cx);
                }
                if std::mem::take(&mut this.pending) {
                    this.reload(cx);
                }
            },
        ));
    }

    /// Re-renders once a second while the task runs (live duration).
    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticking || !self.is_running() {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let keep = this
                .update(cx, |this, cx| {
                    let running = this.is_running();
                    if running {
                        cx.notify();
                    } else {
                        this.ticking = false;
                    }
                    running
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        })
        .detach();
    }

    fn is_running(&self) -> bool {
        self.data
            .as_ref()
            .and_then(|d| d.task.as_ref())
            .is_some_and(|t| t.status == Status::Running)
    }

    fn task(&self) -> Option<&CoreTask> {
        self.data.as_ref().and_then(|d| d.task.as_ref())
    }

    /// Posts the composer text into the task's thread.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.task() else { return };
        if !can_steer(task) {
            return;
        }
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let id = task.id.clone();
        self.composer
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.stick = true;
        state::run_engine(cx, move |e| e.steer(&id, &text)).detach();
        cx.notify();
    }

    fn toggle(&mut self, id: SharedString, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        cx.notify();
    }

    // ---- rendering ----------------------------------------------------------------

    fn header(&self, t: &CoreTask, cx: &mut Context<Self>) -> PageHeader {
        let back = IconButton::new("task-back", AppIcon::ArrowLeft)
            .tooltip("Back")
            .on_click(|_, _, cx| state::back(cx));
        let now = Utc::now();
        let mut pill = StatusPill::new(t.status).id("task-status");
        if t.status == Status::Running {
            pill = pill.suffix(ui::fmt_duration(
                (now - t.started_at.unwrap_or(t.created_at)).num_seconds(),
            ));
        }
        let mut h = PageHeader::new(t.title.clone()).meta(back).meta(pill);
        if !t.model.is_empty() {
            h = h.meta(Badge::new(Config::short_model(&t.model)));
        }
        if t.kind == TaskKind::Spawned {
            h = h.meta(Badge::new(format!("sub-agent · depth {}", t.depth)).spawn());
        }
        if let Some(v) = t.variant {
            h = h.meta(VariantChip::variant(v));
        }
        let id = t.id.clone();
        if t.status == Status::Waiting {
            let tid = id.clone();
            h = h.action(
                Button::new("task-review", "Open in Review")
                    .primary()
                    .icon_right(AppIcon::ChevronRight)
                    .on_click(move |_, _, cx| {
                        state::navigate(
                            Route::Review {
                                task: Some(tid.clone()),
                            },
                            cx,
                        )
                    }),
            );
        }
        if matches!(t.status, Status::Failed | Status::Cancelled)
            && t.run_id.is_some()
            && t.node_id.is_some()
        {
            let tid = id.clone();
            h = h.action(
                Button::new("task-retry", "Retry")
                    .icon(AppIcon::RotateCcw)
                    .on_click(move |_, _, cx| {
                        let tid = tid.clone();
                        state::run_engine(cx, move |e| e.retry(&tid)).detach();
                        state::toast(ToastKind::Info, "Retrying the step", cx);
                    }),
            );
        }
        if t.status.is_active() {
            let tid = id;
            h = h.action(
                Button::new("task-cancel", "Cancel")
                    .danger()
                    .icon(AppIcon::X)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let tid = tid.clone();
                        state::run_engine(cx, move |e| e.cancel_task(&tid)).detach();
                        state::toast(ToastKind::Info, "Cancelling the task", cx);
                    })),
            );
        }
        h
    }

    fn breadcrumb(&self, d: &ThreadData) -> impl IntoElement {
        let sep =
            || ui::icon_sized(AppIcon::ChevronRight, px(12.)).text_color(theme::TEXT_DISABLED);
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme::S2)
            .text_label();
        for (i, c) in d.crumbs.iter().enumerate() {
            if i > 0 {
                row = row.child(sep());
            }
            row = row.child(match c.route.clone() {
                Some(route) => div()
                    .id(SharedString::from(format!("task-crumb-{i}")))
                    .fg_accent()
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme::ACCENT))
                    .child(c.label.clone())
                    .on_click(move |_, _, cx| state::navigate(route.clone(), cx))
                    .into_any_element(),
                None => div()
                    .fg_secondary()
                    .child(c.label.clone())
                    .into_any_element(),
            });
        }
        row
    }

    /// Actor, model, folder and branch (mono, click to copy).
    fn meta_row(&self, t: &CoreTask, d: &ThreadData) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(theme::S3)
            .px(theme::S6)
            .py(theme::S3)
            .border_b_1()
            .border_color(theme::LINE)
            .child(self.breadcrumb(d))
            .child(div().flex_1())
            .when(!t.actor.is_empty(), |r| {
                r.child(meta_item("Actor", t.actor.clone()))
            })
            .when(!t.model.is_empty(), |r| {
                r.child(meta_item("Model", t.model.clone()))
            })
            .when_some(t.cwd.clone(), |r, p| {
                r.child(copy_chip("task-cwd", AppIcon::Folder, p))
            })
            .when_some(t.branch.clone(), |r, b| {
                r.child(copy_chip("task-branch", AppIcon::GitBranch, b))
            })
    }

    fn stats(&self, t: &CoreTask, d: &ThreadData) -> impl IntoElement {
        let now = Utc::now();
        let end = t.finished_at.unwrap_or(now);
        let dur = t
            .started_at
            .map(|s| ui::fmt_duration((end - s).num_seconds()))
            .unwrap_or_else(|| "–".into());
        let (files, added, removed) = output_stats(&t.output);
        let running_kids = d.children.iter().filter(|c| c.status.is_active()).count();
        let cell = |label: &'static str, value: String, note: Option<String>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(theme::S6)
                .py(theme::S3)
                .border_r_1()
                .border_color(theme::LINE)
                .child(div().text_label().fg_muted().child(label))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(theme::S2)
                        .child(div().text_stat_sm().fg().child(value))
                        .when_some(note, |r, n| r.child(div().text_label().fg_muted().child(n))),
                )
        };
        div()
            .flex()
            .flex_none()
            .border_b_1()
            .border_color(theme::LINE)
            .child(cell("Spend", ui::fmt_usd(t.cost_usd), None))
            .child(cell(
                "Tokens",
                fmt_tokens(t.tokens_in + t.tokens_out),
                Some(format!(
                    "{} in · {} out",
                    fmt_tokens(t.tokens_in),
                    fmt_tokens(t.tokens_out)
                )),
            ))
            .child(cell("Steps", t.steps.to_string(), None))
            .child(cell("Duration", dur, None))
            .child(cell(
                "Changed",
                format!("{files} file{}", if files == 1 { "" } else { "s" }),
                Some(format!("+{added} −{removed}")),
            ))
            .child(cell(
                "Spawned",
                d.children.len().to_string(),
                (running_kids > 0).then(|| format!("{running_kids} running")),
            ))
    }

    fn entry(
        &self,
        e: &Entry,
        running: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match e {
            Entry::System { id, text, time } => div()
                .id(id.clone())
                .flex()
                .items_start()
                .gap(theme::S2)
                .text_mono()
                .fg_muted()
                .child(div().flex_none().child("—"))
                .child(div().flex_none().child(time.clone()))
                .child(div().flex_1().min_w_0().child(text.clone()))
                .into_any_element(),
            Entry::User {
                id,
                text,
                author,
                time,
                steer,
                delivered,
            } => {
                let mine = *steer;
                div()
                    .id(id.clone())
                    .flex()
                    .flex_col()
                    .gap(theme::S1)
                    .when(mine, |d| d.items_end())
                    .child(
                        div()
                            .flex()
                            .gap(theme::S2)
                            .text_label()
                            .child(
                                div()
                                    .fg_secondary()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(if mine {
                                        SharedString::from("You")
                                    } else {
                                        author.clone()
                                    }),
                            )
                            .child(div().fg_muted().child(time.clone())),
                    )
                    .child(
                        div()
                            .max_w(px(640.))
                            .px(theme::S4)
                            .py(theme::S3)
                            .rounded(theme::RADIUS_LG)
                            .border_1()
                            .text_body()
                            .fg()
                            .when(mine, |d| {
                                d.bg(theme::ACCENT_SOFT).border_color(theme::ACCENT_RING)
                            })
                            .when(!mine, |d| {
                                d.bg(theme::SURFACE).border_color(theme::LINE_STRONG)
                            })
                            .child(text.clone()),
                    )
                    .when(mine, |d| {
                        d.child(div().text_label().fg_accent().child(if *delivered {
                            "seen by the agent"
                        } else {
                            "lands in the agent's next turn"
                        }))
                    })
                    .into_any_element()
            }
            Entry::Assistant {
                id,
                text,
                author,
                time,
                partial,
            } => div()
                .id(id.clone())
                .flex()
                .flex_col()
                .gap(theme::S1)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .text_label()
                        .child(VariantChip::initial(author).size(px(20.)))
                        .child(
                            div()
                                .fg_secondary()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(author.clone()),
                        )
                        .child(div().fg_muted().child(time.clone())),
                )
                .when(!text.is_empty(), |d| {
                    d.child(ui::markdown(
                        SharedString::from(format!("msg-{id}")),
                        text.clone(),
                        window,
                        cx,
                    ))
                })
                .when(*partial, |d| d.child(caret(id.clone())))
                .into_any_element(),
            Entry::Tool {
                id,
                name,
                summary,
                args,
                result,
                failed,
            } => {
                let open = self.expanded.contains(id);
                let pending = result.is_none() && running;
                let key = id.clone();
                let accent = if *failed {
                    theme::DANGER
                } else {
                    theme::ACCENT_TEXT
                };
                div()
                    .flex()
                    .flex_col()
                    .rounded(theme::RADIUS_LG)
                    .border_1()
                    .border_color(if *failed {
                        theme::alpha(theme::DANGER, 0.35)
                    } else {
                        theme::LINE
                    })
                    .bg(if *failed {
                        theme::DANGER_SOFT
                    } else {
                        theme::SURFACE
                    })
                    .child(
                        div()
                            .id(id.clone())
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .px(theme::S3)
                            .py(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::SURFACE_RAISED))
                            .text_mono()
                            .child(
                                ui::icon_sized(
                                    if open {
                                        AppIcon::ChevronDown
                                    } else {
                                        AppIcon::ChevronRight
                                    },
                                    theme::ICON_SM,
                                )
                                .text_color(theme::TEXT_MUTED),
                            )
                            .child(div().flex_none().text_color(accent).child(name.clone()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .fg_muted()
                                    .child(summary.clone()),
                            )
                            .child(if pending {
                                StatusDot::new(Status::Running)
                                    .id(SharedString::from(format!("tool-dot-{id}")))
                                    .small()
                                    .into_any_element()
                            } else if *failed {
                                div()
                                    .flex_none()
                                    .text_color(theme::DANGER)
                                    .child("failed")
                                    .into_any_element()
                            } else if result.is_some() {
                                ui::icon_sized(AppIcon::Check, theme::ICON_SM)
                                    .text_color(theme::SUCCESS)
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle(key.clone(), cx)),
                            ),
                    )
                    .when(open, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(theme::S2)
                                .p(theme::S3)
                                .border_t_1()
                                .border_color(theme::LINE)
                                .child(div().text_label().fg_muted().child("Arguments"))
                                .child(
                                    CodeBlock::new(
                                        SharedString::from(format!("{id}-args")),
                                        args.clone(),
                                    )
                                    .max_height(px(220.)),
                                )
                                .child(div().text_label().fg_muted().child("Result"))
                                .child(match result {
                                    Some(r) => CodeBlock::new(
                                        SharedString::from(format!("{id}-res")),
                                        r.clone(),
                                    )
                                    .max_height(px(280.))
                                    .into_any_element(),
                                    None => div()
                                        .text_label()
                                        .fg_muted()
                                        .child(if running {
                                            "Running…"
                                        } else {
                                            "No result recorded."
                                        })
                                        .into_any_element(),
                                })
                                .with_animation(
                                    SharedString::from(format!("{id}-open")),
                                    Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                                    |d, t| d.opacity(t).mt(px(-6.) * (1.0 - t)),
                                ),
                        )
                    })
                    .into_any_element()
            }
        }
    }

    fn thread(
        &mut self,
        d: &ThreadData,
        t: &CoreTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let running = t.status == Status::Running;
        let total = d.entries.len();
        let skip = total.saturating_sub(self.shown);
        let streaming = d
            .entries
            .last()
            .is_some_and(|e| matches!(e, Entry::Assistant { partial: true, .. }));
        if self.stick {
            self.scroll.scroll_to_bottom();
        }

        let mut col = div()
            .flex()
            .flex_col()
            .gap(theme::S4)
            .max_w(px(860.))
            .mx_auto()
            .w_full();
        if skip > 0 {
            col = col.child(
                Button::new(
                    "task-earlier",
                    format!("Show {} earlier messages", skip.min(PAGE)),
                )
                .small()
                .ghost()
                .icon(AppIcon::ChevronDown)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.shown += PAGE;
                    cx.notify();
                })),
            );
        }
        if total == 0 {
            col = col.child(
                EmptyState::new(
                    AppIcon::Chat,
                    "No messages yet",
                    "The agent's messages, tool calls and your notes appear here.",
                )
                .compact()
                .id("task-empty"),
            );
        }
        for e in d.entries.iter().skip(skip) {
            col = col.child(self.entry(e, running, window, cx));
        }
        if running && !streaming {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .text_label()
                    .fg_muted()
                    .child(StatusDot::new(Status::Running).id("task-working").small())
                    .child("Working…"),
            );
        }
        if let Some(err) = t.error.clone().filter(|_| t.status == Status::Failed) {
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::S2)
                    .p(theme::S4)
                    .rounded(theme::RADIUS_XL)
                    .border_1()
                    .border_color(theme::alpha(theme::DANGER, 0.35))
                    .bg(theme::DANGER_SOFT)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .text_body_strong()
                            .text_color(theme::DANGER)
                            .child(ui::icon_sized(AppIcon::Alert, theme::ICON))
                            .child("This task failed"),
                    )
                    .child(div().text_mono().fg_secondary().child(err)),
            );
        }
        if t.status == Status::Done && !t.summary.trim().is_empty() {
            let (files, added, removed) = output_stats(&t.output);
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::S2)
                    .p(theme::S4)
                    .rounded(theme::RADIUS_XL)
                    .border_1()
                    .border_color(theme::alpha(theme::SUCCESS, 0.35))
                    .bg(theme::SURFACE)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .child(
                                div()
                                    .text_color(theme::SUCCESS)
                                    .child(ui::icon_sized(AppIcon::CircleCheck, theme::ICON)),
                            )
                            .child(div().text_body_strong().fg().child("Summary"))
                            .child(div().flex_1())
                            .when(files + added + removed > 0, |r| {
                                r.child(
                                    div()
                                        .flex()
                                        .gap(theme::S2)
                                        .text_mono()
                                        .child(div().fg_muted().child(format!(
                                            "{files} file{}",
                                            if files == 1 { "" } else { "s" }
                                        )))
                                        .child(
                                            div()
                                                .text_color(theme::DIFF_ADD_TEXT)
                                                .child(format!("+{added}")),
                                        )
                                        .child(
                                            div()
                                                .text_color(theme::DIFF_DEL_TEXT)
                                                .child(format!("−{removed}")),
                                        ),
                                )
                            }),
                    )
                    .child(ui::markdown(
                        SharedString::from(format!("summary-{}", t.id)),
                        t.summary.clone(),
                        window,
                        cx,
                    )),
            );
        }

        let at_bottom = self.stick;
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("task-thread-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(theme::S6)
                    .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, window, cx| {
                        // After the scroll is applied: stick again only when back at the end.
                        let me = cx.entity();
                        window.on_next_frame(move |_, cx| {
                            me.update(cx, |this, cx| {
                                let max = this.scroll.max_offset().height;
                                let at_end = this.scroll.offset().y <= -max + px(4.);
                                if this.stick != at_end {
                                    this.stick = at_end;
                                    cx.notify();
                                }
                            });
                        });
                        let _ = this;
                    }))
                    .child(col),
            )
            .when(!at_bottom, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom(theme::S4)
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            Button::new("task-jump", "Jump to latest")
                                .small()
                                .primary()
                                .icon(AppIcon::ChevronDown)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.stick = true;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn composer_area(&self, t: &CoreTask) -> impl IntoElement {
        let wrap = || {
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(theme::S2)
                .px(theme::S6)
                .py(theme::S4)
                .border_t_1()
                .border_color(theme::LINE)
        };
        if t.kind == TaskKind::Chat {
            let chat = t.chat_id.clone();
            return wrap()
                .child(
                    div()
                        .text_body()
                        .fg_muted()
                        .child("This is a chat conversation. Continue it in Chat."),
                )
                .child(
                    div().flex().child(
                        Button::new("task-open-chat", "Open chat")
                            .icon_right(AppIcon::ChevronRight)
                            .on_click(move |_, _, cx| {
                                state::navigate(Route::Chat { chat: chat.clone() }, cx)
                            }),
                    ),
                );
        }
        if !can_steer(t) {
            return wrap().child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .text_body()
                    .fg_muted()
                    .child(ui::icon_sized(AppIcon::Lock, theme::ICON))
                    .child(format!("This task is {}. Steering works only while it is queued, running or waiting.", t.status.label().to_lowercase())),
            );
        }
        let who = if t.actor.is_empty() {
            "the agent".to_string()
        } else {
            t.actor.clone()
        };
        wrap()
            .child(div().text_label().fg_muted().child(format!(
                "Message {who} · it reads this before its next step"
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .child(div().flex_1().child(input::field(&self.composer)))
                    .child(
                        Button::new("task-send", "Send")
                            .primary()
                            .icon(AppIcon::Send)
                            .kbd("↵"),
                    ),
            )
    }

    /// Parent link and spawned sub-tasks.
    fn lineage(&self, t: &CoreTask, d: &ThreadData) -> impl IntoElement {
        let row = |r: &Rel, tag: &'static str| {
            let id = r.id.clone();
            div()
                .id(SharedString::from(format!("task-rel-{tag}-{}", r.id)))
                .flex()
                .items_center()
                .gap(theme::S2)
                .px(theme::S3)
                .py(theme::S2)
                .rounded(theme::RADIUS_MD)
                .cursor_pointer()
                .hover(|s| s.bg(theme::SURFACE_RAISED))
                .child(
                    StatusDot::new(r.status)
                        .id(SharedString::from(format!("task-rel-dot-{tag}-{}", r.id)))
                        .small(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().text_body().fg().child(r.title.clone()))
                        .child(div().truncate().text_label().fg_muted().child(format!(
                            "{}{}{}",
                            r.actor,
                            if r.actor.is_empty() { "" } else { " · " },
                            r.status.label()
                        ))),
                )
                .child(
                    div()
                        .flex_none()
                        .text_mono_sm()
                        .fg_muted()
                        .child(ui::fmt_usd(r.cost)),
                )
                .on_click(move |_, _, cx| state::open_task(id.clone(), cx))
        };
        let section = |title: &'static str| {
            div()
                .px(theme::S3)
                .pt(theme::S2)
                .text_label()
                .fg_muted()
                .child(title)
        };
        div()
            .id("task-lineage")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(300.))
            .gap(theme::S1)
            .p(theme::S3)
            .border_l_1()
            .border_color(theme::LINE)
            .overflow_y_scroll()
            .child(
                div()
                    .px(theme::S3)
                    .pt(theme::S2)
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .text_heading()
                    .fg()
                    .child("Lineage")
                    .child(div().text_label().fg_muted().child("how it got here")),
            )
            .when_some(d.parent.as_ref(), |c, p| {
                c.child(section("Made by")).child(row(p, "parent"))
            })
            .child(section("This task"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .px(theme::S3)
                    .py(theme::S2)
                    .rounded(theme::RADIUS_MD)
                    .bg(theme::SURFACE_RAISED)
                    .child(StatusDot::new(t.status).id("task-rel-dot-self").small())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_body()
                            .fg()
                            .child(t.title.clone()),
                    )
                    .child(div().text_label().fg_muted().child("this thread")),
            )
            .child(section(if d.children.is_empty() {
                "Spawned"
            } else {
                "Spawned sub-tasks"
            }))
            .children(d.children.iter().map(|c| row(c, "child")))
            .when(d.children.is_empty(), |c| {
                c.child(
                    div()
                        .px(theme::S3)
                        .py(theme::S2)
                        .text_label()
                        .fg_disabled()
                        .child("No sub-agents spawned by this task."),
                )
            })
    }
}

impl Render for TaskScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let back = IconButton::new("task-back", AppIcon::ArrowLeft)
            .tooltip("Back")
            .on_click(|_, _, cx| state::back(cx));
        // Taken out so the thread can borrow `self` mutably while rendering.
        let Some(data) = self.data.take() else {
            return screen()
                .child(PageHeader::new("Task").meta(back))
                .child(body().child(skeleton_rows("task-loading", 5)))
                .into_any_element();
        };
        let out = match &data.task {
            None => screen()
                .child(PageHeader::new("Task").meta(back))
                .child(body().child(EmptyState::new(
                    AppIcon::Alert,
                    "Task not found",
                    "It may have been removed from the store.",
                )))
                .into_any_element(),
            Some(t) => {
                let header = self.header(t, cx);
                let meta = self.meta_row(t, &data);
                let stats = self.stats(t, &data);
                let thread = self.thread(&data, t, window, cx);
                let composer = self.composer_area(t);
                let lineage = self.lineage(t, &data);
                screen()
                    .child(header)
                    .child(meta)
                    .child(stats)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(thread)
                                    .child(composer),
                            )
                            .child(lineage),
                    )
                    .into_any_element()
            }
        };
        self.data = Some(data);
        out
    }
}

// ---- data ---------------------------------------------------------------------------

fn can_steer(t: &CoreTask) -> bool {
    t.status.is_active() && t.kind != TaskKind::Chat
}

/// Loads a task, its breadcrumb, lineage and thread.
fn query(s: &Store, id: &str) -> ThreadData {
    let Some(task) = s.task(id) else {
        return ThreadData {
            task: None,
            thread_id: id.to_string(),
            entries: vec![],
            crumbs: vec![],
            parent: None,
            children: vec![],
        };
    };
    let thread_id = if task.kind == TaskKind::Chat {
        task.chat_id.clone().unwrap_or_else(|| task.id.clone())
    } else {
        task.id.clone()
    };
    let entries = entries(&s.messages(&thread_id), &task);

    let mut crumbs = vec![Crumb {
        label: "Board".into(),
        route: Some(Route::Board),
    }];
    if let Some(run) = task.run_id.as_deref().and_then(|r| s.run(r)) {
        let route = Route::Pipelines {
            pipeline: Some(run.pipeline_id.clone()),
        };
        if let Some(p) = s.pipeline(&run.pipeline_id) {
            crumbs.push(Crumb {
                label: p.name.into(),
                route: Some(route.clone()),
            });
        }
        crumbs.push(Crumb {
            label: format!("Run #{}", run.number).into(),
            route: Some(route),
        });
        if let Some(n) = task.node_id.as_deref().and_then(|n| run.graph.node(n)) {
            crumbs.push(Crumb {
                label: n.name.clone().into(),
                route: None,
            });
        }
    } else if task.kind == TaskKind::Chat {
        let title = task
            .chat_id
            .as_deref()
            .and_then(|c| s.chat(c))
            .map(|c| c.title)
            .unwrap_or_else(|| "Chat".into());
        crumbs.push(Crumb {
            label: title.into(),
            route: Some(Route::Chat {
                chat: task.chat_id.clone(),
            }),
        });
    }
    let rel = |t: &CoreTask| Rel {
        id: t.id.clone(),
        title: t.title.clone().into(),
        actor: t.actor.clone().into(),
        status: t.status,
        cost: t.cost_usd,
    };
    let parent = task.parent_id.as_deref().and_then(|p| s.task(p));
    if let Some(p) = &parent {
        crumbs.push(Crumb {
            label: p.title.clone().into(),
            route: Some(Route::Task { task: p.id.clone() }),
        });
    }
    let mut children = s.children(&task.id);
    children.sort_by_key(|c| c.created_at);
    ThreadData {
        parent: parent.as_ref().map(rel),
        children: children.iter().map(rel).collect(),
        entries,
        crumbs,
        thread_id,
        task: Some(task),
    }
}

/// Folds raw messages into display rows: tool calls are paired with their results.
fn entries(messages: &[Message], task: &CoreTask) -> Vec<Entry> {
    let results: std::collections::HashMap<&str, &Message> = messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m)))
        .collect();
    let called: HashSet<&str> = messages
        .iter()
        .flat_map(|m| m.tool_calls.iter().map(|c| c.id.as_str()))
        .filter(|id| results.contains_key(id))
        .collect();
    let time = |m: &Message| {
        SharedString::from(
            m.created_at
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string(),
        )
    };
    let mut out = Vec::new();
    for m in messages {
        match m.role {
            Role::User => out.push(Entry::User {
                id: format!("u-{}", m.id).into(),
                text: m.content.clone().into(),
                author: if m.author.is_empty() {
                    "You".into()
                } else {
                    m.author.clone().into()
                },
                time: time(m),
                steer: m.author.is_empty() || m.author == "you",
                delivered: m.delivered,
            }),
            Role::System => out.push(Entry::System {
                id: format!("s-{}", m.id).into(),
                text: m.content.clone().into(),
                time: time(m),
            }),
            Role::Assistant => {
                if !m.content.trim().is_empty() || m.partial {
                    let author = if m.author.is_empty() {
                        task.actor.clone()
                    } else {
                        m.author.clone()
                    };
                    out.push(Entry::Assistant {
                        id: format!("a-{}", m.id).into(),
                        text: m.content.clone().into(),
                        author: if author.is_empty() {
                            "Agent".into()
                        } else {
                            author.into()
                        },
                        time: time(m),
                        partial: m.partial,
                    });
                }
                for c in &m.tool_calls {
                    let result = results.get(c.id.as_str()).map(|r| r.content.clone());
                    out.push(Entry::Tool {
                        id: format!("t-{}-{}", m.id, c.id).into(),
                        name: c.name.clone().into(),
                        summary: args_summary(&c.arguments).into(),
                        args: pretty_json(&c.arguments).into(),
                        failed: result.as_deref().is_some_and(|r| r.starts_with("Error")),
                        result: result.map(Into::into),
                    });
                }
            }
            Role::Tool => {
                // Results of known calls are folded into their call above.
                if m.tool_call_id
                    .as_deref()
                    .is_some_and(|id| called.contains(id))
                {
                    continue;
                }
                let (name, args) = m
                    .tool_calls
                    .first()
                    .map(|c| (c.name.clone(), c.arguments.clone()))
                    .unwrap_or((m.author.clone(), String::new()));
                out.push(Entry::Tool {
                    id: format!("t-{}", m.id).into(),
                    name: if name.is_empty() {
                        "tool".into()
                    } else {
                        name.into()
                    },
                    summary: args_summary(&args).into(),
                    args: pretty_json(&args).into(),
                    failed: m.content.starts_with("Error"),
                    result: Some(m.content.clone().into()),
                });
            }
        }
    }
    out
}

/// The most telling argument on one line: `path`, `command`, ... or the first scalar.
fn args_summary(arguments: &str) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(arguments) else {
        return arguments
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(100)
            .collect();
    };
    let scalar = |v: &Value| match v {
        Value::String(s) => Some(s.lines().next().unwrap_or("").to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    [
        "path", "command", "pattern", "title", "url", "query", "name",
    ]
    .iter()
    .find_map(|k| map.get(*k).and_then(scalar))
    .or_else(|| map.values().find_map(scalar))
    .map(|s| s.chars().take(100).collect())
    .unwrap_or_default()
}

fn pretty_json(arguments: &str) -> String {
    match serde_json::from_str::<Value>(arguments) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| arguments.to_string()),
        Err(_) => arguments.to_string(),
    }
}

/// `(files, added, removed)` from a task's `output` (set by pipeline agent steps).
fn output_stats(out: &Value) -> (u64, u64, u64) {
    let files = match &out["files"] {
        Value::Array(a) => a.len() as u64,
        v => v.as_u64().unwrap_or(0),
    };
    (
        files,
        out["added"].as_u64().unwrap_or(0),
        out["removed"].as_u64().unwrap_or(0),
    )
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

// ---- small pieces -------------------------------------------------------------------

fn meta_item(label: &'static str, value: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(theme::S2)
        .text_label()
        .child(div().fg_muted().child(label))
        .child(div().fg_secondary().child(value))
}

/// Mono chip that copies its text on click.
fn copy_chip(id: &'static str, icon: AppIcon, text: String) -> impl IntoElement {
    let copy = text.clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(theme::S2)
        .h(px(22.))
        .max_w(px(360.))
        .px(theme::S2)
        .rounded(theme::RADIUS_MD)
        .border_1()
        .border_color(theme::LINE_STRONG)
        .bg(theme::SURFACE_RAISED)
        .text_mono()
        .fg_secondary()
        .cursor_pointer()
        .hover(|s| s.border_color(theme::TEXT_DISABLED))
        .child(ui::icon_sized(icon, theme::ICON_SM).text_color(theme::TEXT_MUTED))
        .child(div().min_w_0().truncate().child(text))
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
            state::toast(ToastKind::Info, "Copied to clipboard", cx);
        })
}

/// Blinking streaming cursor shown under a partial assistant message.
fn caret(id: SharedString) -> impl IntoElement {
    div()
        .w(px(7.))
        .h(px(15.))
        .rounded(px(1.))
        .bg(theme::ACCENT)
        .with_animation(
            SharedString::from(format!("caret-{id}")),
            Animation::new(Duration::from_millis(900))
                .repeat()
                .with_easing(pulsating_between(0.15, 1.0)),
            |d, t| d.opacity(t),
        )
}
