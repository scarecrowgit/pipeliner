//! Issue board screen: issues (see the `pipeline` crate) as a board or a list, the "New
//! issue" dialog and the issue detail panel. Each issue runs through one of the configured
//! pipelines; the panel shows its run step by step, whatever the steps are.
//!
//! Modules: [`look`] how pipeline state reads, [`header`] title bar + view tabs, [`board`] and
//! [`list`] the two views, [`dialog`] "New issue", [`menu`] the pipeline select, [`detail`]
//! the panel frame, [`progress`] its step list (decisions, notes), [`run`] the review section
//! (renders, approval) and the render viewer, [`work`] sub-agent tasks, checks, workdirs and
//! the log.

mod board;
mod detail;
mod dialog;
mod header;
mod list;
mod look;
mod menu;
mod progress;
mod run;
mod widgets;
mod work;

use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Render, ScrollHandle,
    SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use pipeline::{Issue, IssueStatus, Pipelines, Run};
use ui::nav::Screen;
use ui::{TextInput, TextInputEvent};

actions!(screen_issues, [CloseDetail]);

const KEY_CONTEXT: &str = "IssuesScreen";

/// Log lines shown in the detail panel (newest at the bottom).
pub(crate) const LOG_LINES: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Board,
    List,
}

/// One issue as the board and list draw it (copied out of the service per render).
#[derive(Clone)]
pub(crate) struct Card {
    pub key: SharedString,
    pub title: SharedString,
    pub kind: SharedString,
    pub status: IssueStatus,
    pub line: look::Line,
    /// Run time so far (`4m`), when there is a run.
    pub elapsed: Option<SharedString>,
    /// Stage progress of a run that is working now.
    pub progress: Option<f32>,
}

/// "New issue" dialog state.
pub(crate) struct NewIssue {
    pub open: bool,
    pub title: Entity<TextInput>,
    pub body: Entity<TextInput>,
    /// The description box hosts the input (else it shows the wrapped text).
    pub body_editing: bool,
    pub kind: usize,
    /// Pipeline id; `None` = the default pipeline.
    pub pipeline: Option<String>,
    pub run_now: bool,
    pub error: Option<SharedString>,
}

pub struct IssuesScreen {
    focus_handle: FocusHandle,
    pipes: Entity<Pipelines>,
    /// Selected issue key (detail panel open when `Some`).
    selected: Option<SharedString>,
    view: View,
    new: NewIssue,
    /// "Request changes…" revealed under the approval buttons.
    changes_open: bool,
    changes_input: Entity<TextInput>,
    /// "Delete?" confirm revealed in the detail actions.
    confirm_delete: bool,
    /// Set when an action on the service failed (shown in the detail panel).
    action_error: Option<SharedString>,
    log_scroll: ScrollHandle,
    /// The detail panel's scroll (back to the top when another issue is selected).
    detail_scroll: ScrollHandle,
    /// (issue, log length) last scrolled to, so new lines scroll the log to the bottom.
    log_seen: Option<(SharedString, usize)>,
    /// Render viewer: (issue, render index).
    viewer: Option<(SharedString, usize)>,
    /// The open pipeline select, if any.
    menu: Option<menu::Menu>,
    /// Steps whose full notes are shown (else the first lines).
    notes_open: HashSet<String>,
    _subscriptions: Vec<Subscription>,
    /// Re-renders once in a while so elapsed times move while runs work.
    _ticker: Task<()>,
}

impl IssuesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new("escape", CloseDetail, Some(KEY_CONTEXT))]);

        // The app shell creates the service; standalone previews get their own.
        let pipes = pipeline::global(cx).unwrap_or_else(|| pipeline::init(cx));

        let title = cx.new(|cx| TextInput::new("Short summary, e.g. Add an export button", cx));
        let body = cx.new(|cx| TextInput::new("What should change, and why", cx));
        let changes_input = cx.new(|cx| TextInput::new("What should change?", cx));

        let mut subscriptions = vec![
            cx.observe(&pipes, |this, _, cx| {
                this.follow_log(cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &title,
                window,
                |this, _, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Changed => {
                        this.new.error = None;
                        cx.notify();
                    }
                    TextInputEvent::Submit | TextInputEvent::Tab => this.edit_body(window, cx),
                    TextInputEvent::Cancel => this.close_new(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &body,
                window,
                |this, _, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Changed => cx.notify(),
                    TextInputEvent::Submit => this.create(window, cx),
                    TextInputEvent::Cancel => this.close_new(window, cx),
                    TextInputEvent::Blur => {
                        this.new.body_editing = false;
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &changes_input,
                window,
                |this, _, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Changed => cx.notify(),
                    TextInputEvent::Submit => this.send_changes(cx),
                    TextInputEvent::Cancel => {
                        this.changes_open = false;
                        this.focus_self(window);
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        ];

        // The app shell builds every screen up-front: only take focus when shown, and
        // re-take it each time navigation lands here so Escape works without a click.
        subscriptions.extend(ui::nav::on_activate(
            Screen::Issues,
            window,
            cx,
            |this: &mut Self, window, _| this.focus_self(window),
        ));
        // Other screens open an issue with `ui::nav::open_issue`.
        if let Some(nav) = ui::nav::nav_entity(cx) {
            subscriptions.push(cx.observe_in(&nav, window, |this: &mut Self, _, window, cx| {
                if let Some(key) = ui::nav::take_issue_focus(cx)
                    && this.pipes.read(cx).issue(&key).is_some()
                {
                    this.select(key, window, cx);
                }
            }));
        }
        let focus_handle = cx.focus_handle();
        let shown = ui::nav::nav_entity(cx).is_none() || ui::nav::current(cx) == Screen::Issues;
        if shown {
            window.focus(&focus_handle);
        }

        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(15))
                    .await;
                let ok = this.update(cx, |this, cx| {
                    let p = this.pipes.read(cx);
                    if p.issues.iter().any(|i| p.is_active(&i.key)) {
                        cx.notify();
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        });

        // Test hooks: `HARNESS_ISSUE=<KEY>` opens an issue (`HARNESS_ISSUE_SCROLL=bottom`
        // scrolls its panel down, `HARNESS_ISSUE_RENDER=<n>` opens render n large), `HARNESS_ISSUES_DIALOG=new` opens the "New issue" dialog,
        // `HARNESS_ISSUES_VIEW=list` starts in the list view.
        let selected = std::env::var("HARNESS_ISSUE")
            .ok()
            .filter(|k| pipes.read(cx).issue(k).is_some())
            .map(SharedString::from);
        let dialog = std::env::var("HARNESS_ISSUES_DIALOG").is_ok_and(|v| v == "new");

        let mut this = Self {
            focus_handle,
            pipes,
            selected,
            view: if std::env::var("HARNESS_ISSUES_VIEW").is_ok_and(|v| v == "list") {
                View::List
            } else {
                View::Board
            },
            new: NewIssue {
                open: false,
                title,
                body,
                body_editing: false,
                kind: 0,
                pipeline: None,
                run_now: true,
                error: None,
            },
            changes_open: false,
            changes_input,
            confirm_delete: false,
            action_error: None,
            log_scroll: ScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            log_seen: None,
            viewer: None,
            menu: None,
            notes_open: HashSet::new(),
            _subscriptions: subscriptions,
            _ticker: ticker,
        };
        this.follow_log(cx);
        if let Ok(ix) = std::env::var("HARNESS_ISSUE_RENDER").map(|v| v.parse().unwrap_or(0)) {
            this.viewer = this.selected.clone().map(|k| (k, ix));
        }
        if std::env::var("HARNESS_ISSUE_SCROLL").is_ok_and(|v| v == "bottom") {
            this.detail_scroll.scroll_to_bottom();
        }
        if dialog {
            this.open_new(window, cx);
        }
        // `HARNESS_ISSUES_MENU=new|issue` opens the dialog's / the panel's pipeline select.
        this.menu = match std::env::var("HARNESS_ISSUES_MENU").as_deref() {
            Ok("new") if dialog => Some(menu::Menu::NewIssue),
            Ok("issue") if this.selected.is_some() => Some(menu::Menu::Issue),
            _ => None,
        };
        // Test hook: `HARNESS_NEW_ISSUE="Title::Description"` creates an issue and runs the
        // pipeline on it, through the same call as the dialog's "Create and run".
        if let Ok(spec) = std::env::var("HARNESS_NEW_ISSUE") {
            let (title, body) = spec.split_once("::").unwrap_or((spec.as_str(), ""));
            let (title, body) = (title.to_string(), body.to_string());
            let pipes = this.pipes.clone();
            cx.defer(move |cx| {
                let key = pipes.update(cx, |p, cx| p.create_issue(&title, &body, "Feature", None, true, cx));
                if let Ok(key) = key {
                    ui::nav::open_issue(key, cx);
                }
            });
        }
        this
    }

    // ── Queries ─────────────────────────────────────────────────────────

    /// Board/list rows from the service, newest first.
    fn cards(&self, cx: &App) -> Vec<Card> {
        let p = self.pipes.read(cx);
        let now = ui::time::now_secs();
        p.issues
            .iter()
            .map(|i| {
                let active = p.is_active(&i.key);
                let (line, elapsed, progress) = p
                    .with_run(&i.key, |r| {
                        (
                            look::status_line(i, Some(r), active),
                            Some(look::duration_short(look::run_elapsed(r, active, now)).into()),
                            active.then(|| look::run_fraction(r)),
                        )
                    })
                    .unwrap_or_else(|| (look::status_line(i, None, false), None, None));
                Card {
                    key: i.key.clone().into(),
                    title: i.title.clone().into(),
                    kind: i.kind.clone().into(),
                    status: look::column_of(i.status),
                    line,
                    elapsed,
                    progress,
                }
            })
            .collect()
    }

    /// The selected issue, its latest run (copied) and whether a worker runs it.
    fn selection(&self, cx: &App) -> Option<(Issue, Option<Run>, bool)> {
        let key = self.selected.as_ref()?;
        let p = self.pipes.read(cx);
        let issue = p.issue(key)?.clone();
        let run = p.with_run(key, Run::clone);
        Some((issue, run, p.is_active(key)))
    }

    fn open_count(&self, cx: &App) -> usize {
        let p = self.pipes.read(cx);
        p.issues
            .iter()
            .filter(|i| i.status != IssueStatus::Done)
            .count()
    }

    // ── Focus ───────────────────────────────────────────────────────────

    fn focus_self(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    // ── Selection ───────────────────────────────────────────────────────

    fn select(&mut self, key: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&key) {
            self.changes_open = false;
            self.confirm_delete = false;
            self.action_error = None;
            self.changes_input.update(cx, |i, cx| i.set_text("", cx));
            self.detail_scroll.set_offset(gpui::Point::default());
            self.notes_open.clear();
            self.menu = None;
        }
        self.selected = Some(key);
        self.follow_log(cx);
        self.focus_self(window);
        cx.notify();
    }

    fn close_selection(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.changes_open = false;
        self.confirm_delete = false;
        self.viewer = None;
        self.menu = None;
        cx.notify();
    }

    /// Escape closes the top-most thing: viewer, dialog, change request, then the panel.
    fn close_detail(&mut self, _: &CloseDetail, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.menu = None;
        } else if self.viewer.is_some() {
            self.viewer = None;
        } else if self.new.open {
            self.close_new(window, cx);
        } else if self.changes_open {
            self.changes_open = false;
        } else if self.confirm_delete {
            self.confirm_delete = false;
        } else {
            self.selected = None;
        }
        cx.notify();
    }

    /// Scroll the log to the bottom when the selected run got new lines.
    fn follow_log(&mut self, cx: &App) {
        let Some(key) = self.selected.clone() else {
            self.log_seen = None;
            return;
        };
        let len = self
            .pipes
            .read(cx)
            .with_run(&key, |r| r.log.len())
            .unwrap_or(0);
        if self.log_seen.as_ref() != Some(&(key.clone(), len)) {
            self.log_seen = Some((key, len));
            self.log_scroll.scroll_to_bottom();
        }
    }

    fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    // ── Pipeline commands ───────────────────────────────────────────────

    /// Run `f` on the service for the selected issue.
    fn command(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Pipelines, &str, &mut Context<Pipelines>),
    ) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        self.action_error = None;
        self.confirm_delete = false;
        self.pipes.update(cx, |p, cx| f(p, &key, cx));
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.command(cx, |p, key, cx| p.start(key, cx));
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.command(cx, |p, key, cx| p.cancel(key, cx));
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        self.command(cx, |p, key, cx| p.retry(key, cx));
    }

    fn approve(&mut self, cx: &mut Context<Self>) {
        self.changes_open = false;
        self.command(cx, |p, key, cx| p.approve(key, cx));
    }

    fn toggle_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.changes_open = !self.changes_open;
        if self.changes_open {
            window.focus(&self.changes_input.focus_handle(cx));
        }
        cx.notify();
    }

    fn send_changes(&mut self, cx: &mut Context<Self>) {
        let notes = self.changes_input.read(cx).text().trim().to_string();
        if notes.is_empty() {
            return;
        }
        self.changes_open = false;
        self.changes_input.update(cx, |i, cx| i.set_text("", cx));
        self.command(cx, |p, key, cx| p.request_changes(key, &notes, cx));
    }

    fn ask_delete(&mut self, confirm: bool, cx: &mut Context<Self>) {
        self.confirm_delete = confirm;
        cx.notify();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        self.command(cx, |p, key, cx| p.delete_issue(key, cx));
        let gone = self
            .selected
            .as_ref()
            .is_some_and(|k| self.pipes.read(cx).issue(k).is_none());
        if gone {
            self.close_selection(cx);
        } else {
            self.action_error = Some("The issue is still running; cancel it first.".into());
        }
    }

    fn set_issue_pipeline(&mut self, id: String, cx: &mut Context<Self>) {
        self.menu = None;
        self.command(cx, move |p, key, cx| p.set_issue_pipeline(key, &id, cx));
    }

    fn toggle_notes(&mut self, step: String, cx: &mut Context<Self>) {
        if !self.notes_open.remove(&step) {
            self.notes_open.insert(step);
        }
        cx.notify();
    }

    fn open_viewer(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.viewer = self.selected.clone().map(|k| (k, ix));
        cx.notify();
    }

    fn close_viewer(&mut self, cx: &mut Context<Self>) {
        self.viewer = None;
        cx.notify();
    }
}

impl Focusable for IssuesScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for IssuesScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cards = self.cards(cx);
        let main = if cards.is_empty() {
            self.render_empty(cx).into_any_element()
        } else {
            match self.view {
                View::Board => self.render_board(&cards, cx).into_any_element(),
                View::List => self.render_list(&cards, cx).into_any_element(),
            }
        };
        let detail = self.selection(cx).map(|(issue, run, active)| {
            self.render_detail(&issue, run.as_ref(), active, window, cx)
        });
        let viewer = self.render_viewer(window, cx);
        let dialog = self.render_new(window, cx);
        let menu_backdrop = self.render_menu_backdrop(cx);

        div()
            .id("issues-screen")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_detail))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.render_header(&cards, cx))
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().flex_1().min_w_0().h_full().child(main))
                    .children(detail),
            )
            .children(viewer)
            .children(dialog)
            .children(menu_backdrop)
    }
}
