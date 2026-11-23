//! Review queue ("Inbox") screen: issues waiting for a person to pick one of the
//! agent-produced variants, inspect its diff and ship / skip / send it back.

mod data;
mod report;
mod view;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Global, IntoElement, KeyBinding, MouseButton,
    Render, Subscription, Window, actions, div, prelude::*,
};
use ui::nav::Screen;
use ui::{TextInput, TextInputEvent};

use data::{ISSUES, Issue, Variant};

actions!(
    review_screen,
    [
        MoveDown,
        MoveUp,
        Pick1,
        Pick2,
        Pick3,
        SendBack,
        CancelSendBack
    ]
);

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
        KeyBinding::new("1", Pick1, ctx),
        KeyBinding::new("2", Pick2, ctx),
        KeyBinding::new("3", Pick3, ctx),
        KeyBinding::new("r", SendBack, ctx),
        KeyBinding::new("escape", CancelSendBack, ctx),
    ]);
}

/// What happened to a queue item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Done {
    Shipped,
    /// Sent back with notes; the agent is re-running.
    Back,
}

pub struct ReviewScreen {
    focus: FocusHandle,
    /// Index of the open issue in `ISSUES`.
    cur: usize,
    /// Picked variant per issue.
    picked: Vec<usize>,
    done: Vec<Option<Done>>,
    sending_back: bool,
    /// Send-back notes per issue (kept while switching issues).
    notes: Vec<String>,
    notes_input: Entity<TextInput>,
    /// Agent reports (`screen_chat::inbox`), newest first.
    reports: Vec<screen_chat::inbox::InboxItem>,
    /// The open report; `None` shows the pipeline review `cur`.
    report: Option<String>,
    /// Pull request status by URL.
    prs: report::PrCache,
    inbox_version: u64,
    /// The newest report was opened on the first visit.
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

        let notes_input = cx.new(|cx| TextInput::new("What should the agent change?", cx));
        let mut subscriptions = vec![cx.subscribe_in(&notes_input, window, Self::on_notes_event)];
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
        let reports = screen_chat::inbox::load();
        let unread = screen_chat::inbox::unread(&reports) + ISSUES.len();
        cx.defer(move |cx| ui::nav::set_inbox_count(unread, cx));

        ui::time::refresh_ages(cx).detach();

        Self {
            focus,
            cur: 0,
            picked: vec![0; ISSUES.len()],
            done: vec![None; ISSUES.len()],
            sending_back: false,
            notes: ISSUES.iter().map(|i| i.default_notes.to_string()).collect(),
            notes_input,
            reports,
            report: None,
            prs: Default::default(),
            inbox_version: 0,
            auto_opened: false,
            _subscriptions: subscriptions,
        }
    }

    // ── Derived state ───────────────────────────────────────────────────

    fn issue(&self) -> &'static Issue {
        &ISSUES[self.cur]
    }

    fn picked_index(&self) -> usize {
        self.picked[self.cur]
    }

    fn picked_variant(&self) -> &'static Variant {
        &self.issue().variants[self.picked_index()]
    }

    fn is_shipped(&self) -> bool {
        self.done[self.cur] == Some(Done::Shipped)
    }

    /// Shipped or sent back: the issue can no longer be changed.
    fn is_done(&self) -> bool {
        self.done[self.cur].is_some()
    }

    fn waiting_issues(&self) -> usize {
        self.done.iter().filter(|d| d.is_none()).count()
    }

    fn waiting_variants(&self) -> usize {
        ISSUES
            .iter()
            .zip(&self.done)
            .filter(|(_, d)| d.is_none())
            .map(|(i, _)| i.variants.len())
            .sum()
    }

    // ── Mutations (ported from the mockup logic) ────────────────────────

    fn open(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= ISSUES.len() {
            return;
        }
        self.cur = index;
        self.report = None;
        self.close_send_back(window);
        cx.notify();
    }

    fn pick(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.report.is_none()
            && !self.is_done() && index < self.issue().variants.len() {
            self.picked[self.cur] = index;
            cx.notify();
        }
    }

    /// "Skip" / "Next issue": the next issue (wrapping) that is not done yet.
    fn next_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = ISSUES.len();
        if let Some(j) = (1..=n)
            .map(|k| (self.cur + k) % n)
            .find(|&j| self.done[j].is_none())
        {
            self.open(j, window, cx);
        }
    }

    fn ship(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_done() {
            return;
        }
        self.done[self.cur] = Some(Done::Shipped);
        self.close_send_back(window);
        self.publish_count(cx);
        cx.notify();
    }

    fn start_send_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_done() || self.report.is_some() {
            return;
        }
        self.sending_back = true;
        let text = self.notes[self.cur].clone();
        self.notes_input
            .update(cx, |input, cx| input.set_text(text, cx));
        window.focus(&self.notes_input.focus_handle(cx));
        cx.notify();
    }

    fn confirm_send_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_done() {
            return;
        }
        self.done[self.cur] = Some(Done::Back);
        self.close_send_back(window);
        self.publish_count(cx);
        cx.notify();
    }

    fn cancel_send_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_send_back(window);
        cx.notify();
    }

    /// Hide the notes panel and give keyboard focus back to the screen.
    fn close_send_back(&mut self, window: &mut Window) {
        self.sending_back = false;
        window.focus(&self.focus);
    }

    fn on_notes_event(
        &mut self,
        input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => self.notes[self.cur] = input.read(cx).text().to_string(),
            TextInputEvent::Submit => self.confirm_send_back(window, cx),
            TextInputEvent::Cancel => self.cancel_send_back(window, cx),
            _ => {}
        }
    }

    // ── Keyboard actions ────────────────────────────────────────────────

    /// First visit: open the newest report, if any (only while nothing was picked yet).
    fn auto_open(&mut self, cx: &mut Context<Self>) {
        if self.auto_opened {
            return;
        }
        self.auto_opened = true;
        if self.report.is_none()
            && self.cur == 0
            && let Some(id) = self.first_report_id()
        {
            self.open_report(id, cx);
        }
    }

    fn first_report_id(&self) -> Option<String> {
        self.visible_reports().next().map(|r| r.id.clone())
    }

    /// Queue order: agent reports, then pipeline reviews. `Ok(id)` = report, `Err(ix)` = issue.
    fn queue(&self) -> Vec<Result<String, usize>> {
        self.visible_reports()
            .map(|r| Ok(r.id.clone()))
            .chain((0..ISSUES.len()).map(Err))
            .collect()
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.queue();
        let here = match &self.report {
            Some(id) => q.iter().position(|e| e.as_ref() == Ok(id)),
            None => q.iter().position(|e| *e == Err(self.cur)),
        }
        .unwrap_or(0);
        let next = (here as isize + delta).clamp(0, q.len() as isize - 1) as usize;
        match q[next].clone() {
            Ok(id) => self.open_report(id, cx),
            Err(i) => self.open(i, window, cx),
        }
    }

    fn move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.step(1, window, cx);
    }

    fn move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, window, cx);
    }

    fn pick1(&mut self, _: &Pick1, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(0, cx);
    }

    fn pick2(&mut self, _: &Pick2, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(1, cx);
    }

    fn pick3(&mut self, _: &Pick3, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(2, cx);
    }

    fn send_back(&mut self, _: &SendBack, window: &mut Window, cx: &mut Context<Self>) {
        self.start_send_back(window, cx);
    }

    fn cancel(&mut self, _: &CancelSendBack, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending_back {
            self.cancel_send_back(window, cx);
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
            .on_action(cx.listener(Self::pick1))
            .on_action(cx.listener(Self::pick2))
            .on_action(cx.listener(Self::pick3))
            .on_action(cx.listener(Self::send_back))
            .on_action(cx.listener(Self::cancel))
            // Clicking anywhere (outside the notes field) gives the screen keyboard focus.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.notes_input.read(cx).is_focused(window) {
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
