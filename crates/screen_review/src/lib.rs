//! Inbox: what waits for the user. Mockups the pipeline needs approved (renders before /
//! after, approve or request changes) and reports agents posted when they finished work
//! (pull requests with live status, summaries).

mod approval;
mod report;
mod view;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Global, IntoElement, KeyBinding, MouseButton,
    Render, Subscription, Window, actions, div, prelude::*,
};
use ui::nav::Screen;
use ui::{TextInput, TextInputEvent};

actions!(review_screen, [MoveDown, MoveUp]);

const KEY_CONTEXT: &str = "ReviewScreen";

struct KeysBound;
impl Global for KeysBound {}

/// Screen-level shortcuts. Disabled while a text input has focus so typing notes works.
fn bind_keys(cx: &mut App) {
    if cx.has_global::<KeysBound>() {
        return;
    }
    cx.set_global(KeysBound);
    let ctx = Some("ReviewScreen && !TextInput");
    cx.bind_keys([
        KeyBinding::new("j", MoveDown, ctx),
        KeyBinding::new("down", MoveDown, ctx),
        KeyBinding::new("k", MoveUp, ctx),
        KeyBinding::new("up", MoveUp, ctx),
    ]);
}

/// A queue row: a mockup waiting for approval (issue key) or an agent report (item id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Approval(String),
    Report(String),
}

pub struct ReviewScreen {
    focus: FocusHandle,
    /// Agent reports (`agent_tools::inbox`), newest first.
    reports: Vec<agent_tools::inbox::InboxItem>,
    /// The open report; `None` with `approval` set shows an approval.
    report: Option<String>,
    /// Issue whose mockup approval is open.
    approval: Option<String>,
    /// Pull request status by URL.
    prs: report::PrCache,
    inbox_version: u64,
    /// "Request changes…" is open, with its notes.
    changes_open: bool,
    changes_input: Entity<TextInput>,
    pipes: Option<Entity<pipeline::Pipelines>>,
    /// The first entry was opened on the first visit.
    auto_opened: bool,
    _subscriptions: Vec<Subscription>,
}

impl ReviewScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
        let focus = cx.focus_handle();
        // The shell builds every screen up-front: only grab focus when actually shown.
        if ui::nav::nav_entity(cx).is_none() || ui::nav::current(cx) == Screen::Inbox {
            window.focus(&focus);
        }
        let changes_input =
            cx.new(|cx| TextInput::new("What should the designer change?", cx));
        let mut subscriptions = vec![cx.subscribe_in(&changes_input, window, Self::on_changes_event)];
        subscriptions.extend(ui::nav::on_activate(
            Screen::Inbox,
            window,
            cx,
            |this, window, cx| {
                window.focus(&this.focus);
                this.auto_open(cx);
            },
        ));
        // Reports posted by agents (nav.inbox_version) and "open this report" requests.
        if let Some(nav) = ui::nav::nav_entity(cx) {
            subscriptions.push(cx.observe(&nav, |this: &mut Self, nav, cx| {
                let v = nav.read(cx).inbox_version;
                if v != this.inbox_version {
                    this.inbox_version = v;
                    this.reload_reports(cx);
                }
                if let Some(id) = ui::nav::take_inbox_focus(cx) {
                    this.reload_reports(cx);
                    this.open_report(id.to_string(), cx);
                }
            }));
        }
        // Approvals come and go with pipeline runs.
        let pipes = pipeline::global(cx);
        if let Some(p) = &pipes {
            subscriptions.push(cx.observe(p, |this: &mut Self, _, cx| {
                if let Some(key) = this.approval.clone()
                    && !this.approvals(cx).contains(&key)
                {
                    this.approval = None;
                    this.changes_open = false;
                }
                this.publish_count(cx);
                cx.notify();
            }));
        }
        let reports = agent_tools::inbox::load();
        let this = Self {
            focus,
            reports,
            report: None,
            approval: None,
            prs: Default::default(),
            inbox_version: 0,
            changes_open: false,
            changes_input,
            pipes,
            auto_opened: false,
            _subscriptions: subscriptions,
        };
        this.publish_count(cx);
        this
    }

    /// Issues whose mockup waits for the user's approval (newest first).
    pub(crate) fn approvals(&self, cx: &App) -> Vec<String> {
        let Some(p) = &self.pipes else { return Vec::new() };
        let p = p.read(cx);
        p.issues
            .iter()
            .filter(|i| {
                !p.is_active(&i.key)
                    && p.with_run(&i.key, |r| r.status == pipeline::RunStatus::WaitingApproval)
                        == Some(true)
            })
            .map(|i| i.key.clone())
            .collect()
    }

    /// Queue order: approvals, then reports.
    fn queue(&self, cx: &App) -> Vec<Entry> {
        self.approvals(cx)
            .into_iter()
            .map(Entry::Approval)
            .chain(self.visible_reports().map(|r| Entry::Report(r.id.clone())))
            .collect()
    }

    fn current(&self) -> Option<Entry> {
        match (&self.approval, &self.report) {
            (Some(k), _) => Some(Entry::Approval(k.clone())),
            (None, Some(id)) => Some(Entry::Report(id.clone())),
            _ => None,
        }
    }

    pub(crate) fn open_entry(&mut self, e: Entry, cx: &mut Context<Self>) {
        match e {
            Entry::Approval(k) => self.open_approval(k, cx),
            Entry::Report(id) => self.open_report(id, cx),
        }
    }

    pub(crate) fn open_approval(&mut self, key: String, cx: &mut Context<Self>) {
        if self.approval.as_ref() != Some(&key) {
            self.changes_open = false;
            self.changes_input.update(cx, |i, cx| i.set_text("", cx));
        }
        self.report = None;
        self.approval = Some(key);
        cx.notify();
    }

    /// First visit: open the first entry, if any.
    fn auto_open(&mut self, cx: &mut Context<Self>) {
        if self.auto_opened {
            return;
        }
        self.auto_opened = true;
        if self.current().is_none()
            && let Some(e) = self.queue(cx).into_iter().next()
        {
            self.open_entry(e, cx);
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let q = self.queue(cx);
        if q.is_empty() {
            return;
        }
        let here = self
            .current()
            .and_then(|c| q.iter().position(|e| *e == c))
            .unwrap_or(0);
        let next = (here as isize + delta).clamp(0, q.len() as isize - 1) as usize;
        self.open_entry(q[next].clone(), cx);
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn on_changes_event(
        &mut self,
        _: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Submit => self.send_changes(cx),
            TextInputEvent::Cancel => {
                self.changes_open = false;
                window.focus(&self.focus);
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Focusable for ReviewScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReviewScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Started on the Inbox (no activation event): same first-visit behaviour.
        if !self.auto_opened && ui::nav::current(cx) == Screen::Inbox {
            self.auto_open(cx);
        }
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_up))
            // Clicking anywhere (outside the notes field) gives the screen keyboard focus.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.changes_input.read(cx).is_focused(window) {
                        window.focus(&this.focus);
                    }
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.render_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_queue(cx))
                    .child(self.render_main(cx)),
            )
    }
}
