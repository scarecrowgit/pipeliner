//! Issue board screen: header, Board/List views with runner filters, and the
//! issue detail panel (status menu, runner picker, activity + comments).

mod attach;
mod board;
mod data;
mod detail;
mod header;
mod list;
mod picker;
mod widgets;

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Pixels,
    Point, Render, SharedString, Subscription, Window, actions, div, prelude::*,
};
use ui::images::ImageAttachment;
use ui::nav::Screen;
use ui::{TextInput, TextInputEvent};

use attach::Loading;
use data::{Activity, Actor, Column, Filter, Issue, Run, RunState, RunnerId, Trigger};

actions!(screen_issues, [CloseDetail]);

const KEY_CONTEXT: &str = "IssuesScreen";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Board,
    List,
}

pub struct IssuesScreen {
    focus_handle: FocusHandle,
    issues: Vec<Issue>,
    /// Selected issue key (detail panel open when `Some`).
    selected: Option<SharedString>,
    view: View,
    filter: Filter,
    picker_open: bool,
    status_menu_open: bool,
    /// Where the mouse press that dismissed the status menu (outside click) happened,
    /// so the same press landing on the select does not reopen it.
    status_menu_dismissed_at: Option<Point<Pixels>>,
    /// Key of the card being dragged (dimmed while a drag is active).
    dragging: Option<SharedString>,
    next_key: u32,
    title_input: Entity<TextInput>,
    pick_input: Entity<TextInput>,
    comment_input: Entity<TextInput>,
    /// Images waiting to be posted with the next comment (per screen, not per issue).
    comment_images: Vec<(u64, ImageAttachment)>,
    /// Images still being read (comment and description).
    loading: Vec<Loading>,
    comment_error: Option<String>,
    desc_error: Option<String>,
    image_seq: u64,
    paste_seq: usize,
    _subscriptions: Vec<Subscription>,
}

impl IssuesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new("escape", CloseDetail, Some(KEY_CONTEXT))]);

        let title_input = cx.new(|cx| TextInput::new("Title", cx));
        let pick_input = cx.new(|cx| TextInput::new("Orchestrator, agent or pipeline", cx));
        let comment_input =
            cx.new(|cx| TextInput::new("@Fixer try again with the retry budget from ADR-12", cx));

        let mut subscriptions = vec![
            cx.subscribe_in(
                &title_input,
                window,
                |this, input, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Changed => {
                        let text: SharedString = input.read(cx).text().to_string().into();
                        if let Some(issue) = this.selected_issue_mut() {
                            issue.title = text;
                        }
                        cx.notify();
                    }
                    TextInputEvent::Submit | TextInputEvent::Cancel => this.focus_self(window),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &pick_input,
                window,
                |this, _, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Changed => cx.notify(),
                    TextInputEvent::Submit => {
                        if let Some(id) = this.picker_matches(cx).first().copied() {
                            this.attach_runner(id, window, cx);
                        }
                    }
                    TextInputEvent::Cancel => {
                        this.close_picker(cx);
                        this.focus_self(window);
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &comment_input,
                window,
                |this, _, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Changed => cx.notify(),
                    TextInputEvent::Submit => this.post_comment(cx),
                    TextInputEvent::Cancel => this.focus_self(window),
                    TextInputEvent::PastedImages(images) => this.add_pasted(images.clone(), cx),
                    _ => {}
                },
            ),
        ];

        let issues = data::seed();
        let selected: SharedString = "PAY-1440".into();
        if let Some(issue) = issues.iter().find(|i| i.key == selected) {
            let title = issue.title.clone();
            title_input.update(cx, |input, cx| input.set_text(title, cx));
        }

        // The app shell builds every screen up-front: only take focus when shown, and
        // re-take it each time navigation lands here so Escape works without a click.
        subscriptions.extend(ui::nav::on_activate(
            Screen::Issues,
            window,
            cx,
            |this: &mut Self, window, _| this.focus_self(window),
        ));
        let focus_handle = cx.focus_handle();
        if ui::nav::nav_entity(cx).is_none() || ui::nav::current(cx) == Screen::Issues {
            window.focus(&focus_handle);
        }

        Self {
            focus_handle,
            issues,
            selected: Some(selected),
            view: View::Board,
            filter: Filter::All,
            picker_open: true,
            status_menu_open: false,
            status_menu_dismissed_at: None,
            dragging: None,
            next_key: 1443,
            title_input,
            pick_input,
            comment_input,
            comment_images: Vec::new(),
            loading: Vec::new(),
            comment_error: None,
            desc_error: None,
            image_seq: 0,
            paste_seq: 0,
            _subscriptions: subscriptions,
        }
    }

    // ── Queries ─────────────────────────────────────────────────────────

    fn selected_issue(&self) -> Option<&Issue> {
        let key = self.selected.as_ref()?;
        self.issues.iter().find(|i| &i.key == key)
    }

    fn selected_issue_mut(&mut self) -> Option<&mut Issue> {
        let key = self.selected.clone()?;
        self.issue_mut(&key)
    }

    fn issue_mut(&mut self, key: &str) -> Option<&mut Issue> {
        self.issues.iter_mut().find(|i| i.key.as_ref() == key)
    }

    fn visible(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| self.filter.matches(i))
    }

    fn open_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.status != Column::Done)
            .count()
    }

    fn filter_count(&self, filter: Filter) -> usize {
        self.issues.iter().filter(|i| filter.matches(i)).count()
    }

    /// Runner ids matching the picker query, in display (group) order.
    fn picker_matches(&self, cx: &App) -> Vec<RunnerId> {
        let q = self.pick_input.read(cx).text().to_lowercase();
        picker::GROUPS
            .iter()
            .flat_map(|(kind, _)| {
                data::RUNNERS
                    .iter()
                    .enumerate()
                    .filter(move |(_, r)| r.kind == *kind)
            })
            .filter(|(_, r)| {
                q.is_empty() || format!("{} {}", r.name, r.desc).to_lowercase().contains(&q)
            })
            .map(|(id, _)| id)
            .collect()
    }

    // ── Focus ───────────────────────────────────────────────────────────

    fn focus_self(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    // ── Selection ───────────────────────────────────────────────────────

    fn select(&mut self, key: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(issue) = self.issues.iter().find(|i| i.key == key) else {
            return;
        };
        // Reselecting the same unattached issue keeps the picker as it was.
        let keep_picker = issue.runner.is_none() && self.selected.as_ref() == Some(&key);
        let title = issue.title.clone();
        if self.selected.as_ref() != Some(&key) {
            self.reset_pending();
        }
        self.picker_open = keep_picker && self.picker_open;
        self.status_menu_open = false;
        self.selected = Some(key);
        self.reset_picker_query(cx);
        self.title_input
            .update(cx, |input, cx| input.set_text(title, cx));
        self.focus_self(window);
        cx.notify();
    }

    fn close_detail(&mut self, _: &CloseDetail, _: &mut Window, cx: &mut Context<Self>) {
        if self.status_menu_open {
            self.status_menu_open = false;
        } else {
            self.selected = None;
            self.picker_open = false;
        }
        cx.notify();
    }

    fn close_selection(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.picker_open = false;
        self.status_menu_open = false;
        cx.notify();
    }

    // ── Board actions ───────────────────────────────────────────────────

    fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    fn add_issue(&mut self, status: Column, window: &mut Window, cx: &mut Context<Self>) {
        let key = format!("PAY-{}", self.next_key);
        self.next_key += 1;
        let issue = Issue::untitled(key, status);
        let key = issue.key.clone();
        let title = issue.title.clone();
        self.issues.push(issue);
        self.reset_pending();
        self.selected = Some(key);
        self.picker_open = true;
        self.status_menu_open = false;
        self.reset_picker_query(cx);
        // Focus the title with everything selected so typing renames it.
        self.title_input.update(cx, |input, cx| {
            input.set_text(title, cx);
            input.select_all_text(cx);
        });
        window.focus(&self.title_input.focus_handle(cx));
        cx.notify();
    }

    /// Card dropped on a column (the mockup's `col.drop`).
    fn drop_on(
        &mut self,
        key: &SharedString,
        column: Column,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dragging = None;
        if let Some(issue) = self.issue_mut(key) {
            issue.status = column;
            if let Some(r) = issue.runner()
                && column == Column::Progress
                && issue.startable()
            {
                issue.run = Some(Run::running(format!("{} picked it up", r.name), 4));
            }
        }
        self.select(key.clone(), window, cx);
    }

    // ── Detail actions ──────────────────────────────────────────────────

    fn set_status(&mut self, status: Column, cx: &mut Context<Self>) {
        if let Some(issue) = self.selected_issue_mut() {
            issue.status = status;
        }
        self.status_menu_open = false;
        cx.notify();
    }

    fn toggle_status_menu(&mut self, ev: &ClickEvent, cx: &mut Context<Self>) {
        let down = match ev {
            ClickEvent::Mouse(m) => Some(m.down.position),
            ClickEvent::Keyboard(_) => None,
        };
        let dismissed_by_this_press =
            down.is_some() && self.status_menu_dismissed_at.take() == down;
        if !dismissed_by_this_press {
            self.status_menu_open = !self.status_menu_open;
        }
        cx.notify();
    }

    fn dismiss_status_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.status_menu_open {
            self.status_menu_open = false;
            self.status_menu_dismissed_at = Some(position);
            cx.notify();
        }
    }

    fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = !self.picker_open;
        self.reset_picker_query(cx);
        if self.picker_open {
            window.focus(&self.pick_input.focus_handle(cx));
        } else {
            self.focus_self(window);
        }
        cx.notify();
    }

    fn close_picker(&mut self, cx: &mut Context<Self>) {
        self.picker_open = false;
        self.reset_picker_query(cx);
        cx.notify();
    }

    fn reset_picker_query(&mut self, cx: &mut Context<Self>) {
        self.pick_input
            .update(cx, |input, cx| input.set_text("", cx));
    }

    fn attach_runner(&mut self, id: RunnerId, window: &mut Window, cx: &mut Context<Self>) {
        let r = data::runner(id);
        if let Some(issue) = self.selected_issue_mut() {
            issue.runner = Some(id);
            if issue.startable() {
                let text = if issue.status == Column::Backlog {
                    "starts when moved to Todo"
                } else {
                    "starts on assign · runtime cloud"
                };
                issue.run = Some(Run::new(RunState::Queued, text));
            }
            issue.activity.push(Activity::person(
                "ME",
                "now",
                format!("Attached {} {}.", r.kind.label().to_lowercase(), r.name),
            ));
            if issue.status == Column::Backlog {
                issue.status = Column::Todo;
            }
        }
        self.close_picker(cx);
        self.focus_self(window);
    }

    fn toggle_trigger(&mut self, trigger: Trigger, cx: &mut Context<Self>) {
        if let Some(issue) = self.selected_issue_mut() {
            let ix = Trigger::ALL.iter().position(|t| *t == trigger).unwrap_or(0);
            issue.triggers[ix] = !issue.triggers[ix];
        }
        cx.notify();
    }

    fn start_run(&mut self, cx: &mut Context<Self>) {
        if let Some(issue) = self.selected_issue_mut()
            && let Some(r) = issue.runner()
        {
            issue.status = Column::Progress;
            issue.run = Some(Run::running(format!("{} picked it up", r.name), 4));
        }
        cx.notify();
    }

    fn detach(&mut self, cx: &mut Context<Self>) {
        if let Some(issue) = self.selected_issue_mut()
            && let Some(r) = issue.runner()
        {
            issue.runner = None;
            issue.run = None;
            issue.activity.push(Activity::person(
                "ME",
                "now",
                format!("Detached {}.", r.name),
            ));
        }
        cx.notify();
    }

    fn post_comment(&mut self, cx: &mut Context<Self>) {
        let text = self.comment_input.read(cx).text().trim().to_string();
        if !attach::can_post(
            text.is_empty(),
            self.comment_images.len(),
            self.comment_loading(),
        ) {
            return;
        }
        if self.selected_issue().is_none() {
            return;
        }
        let images: Vec<ImageAttachment> = std::mem::take(&mut self.comment_images)
            .into_iter()
            .map(|(_, image)| image)
            .collect();
        self.comment_error = None;
        let Some(issue) = self.selected_issue_mut() else {
            return;
        };
        let mut comment = Activity::person("ME", "now", text.clone());
        comment.images = images;
        issue.activity.push(comment);

        if let Some(id) = mention(&text).and_then(data::runner_by_name) {
            let r = data::runner(id);
            issue.activity.push(Activity::new(
                r.name,
                Actor::of(r.kind),
                "now",
                "Woken by your mention. Picking this up.",
            ));
            issue.runner = issue.runner.or(Some(id));
            issue.run = Some(Run::running(
                format!("{} working on your comment", r.name),
                3,
            ));
            issue.status = Column::Progress;
        } else if let Some(r) = issue.runner()
            && issue
                .run
                .as_ref()
                .is_some_and(|run| run.state == RunState::Blocked)
        {
            issue.activity.push(Activity::agent(
                r.name,
                "now",
                "Thanks, continuing with that scope.",
            ));
            issue.run = Some(Run::running("resumed after your answer", 35));
        }

        self.comment_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }
}

/// First `@word` in `text` (the mockup's `/@(\w+)/`).
fn mention(text: &str) -> Option<&str> {
    let start = text.find('@')? + 1;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

impl Focusable for IssuesScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for IssuesScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.view {
            View::Board => self.render_board(cx).into_any_element(),
            View::List => self.render_list(cx).into_any_element(),
        };
        let detail = self
            .selected_issue()
            .is_some()
            .then(|| self.render_detail(window, cx));

        div()
            .id("issues-screen")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_detail))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.render_header(cx))
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().flex_1().min_w_0().h_full().child(main))
                    .children(detail),
            )
    }
}
