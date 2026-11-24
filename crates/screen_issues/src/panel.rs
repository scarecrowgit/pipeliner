//! The panel of one issue: properties, the pipeline it runs with, its runs, the activity feed
//! and the comment box.
//!
//! Layout, as `issue-board.html` and `issue-comments.html` draw it: a 44px header (key, type,
//! actions, close), then one scrolling column of sections (title and description, properties
//! with "Runs with", the run, the activity feed) and the comment composer docked under it.
//!
//! Areas live in their own modules: [`header`] (key, type, start/stop, more menu), [`props`]
//! (title, description, properties, the runner card), [`pickers`] (the property menus and the
//! runner picker), [`run`] (the latest run and the earlier ones), [`activity`] (the feed) and
//! [`comment`] (the composer: mentions, images, drafts). [`memory`] keeps drafts while the app
//! runs.

mod activity;
mod comment;
mod header;
mod memory;
mod pickers;
mod props;
mod run;

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    ScrollHandle, SharedString, canvas, Subscription, Task, Window, div, prelude::*, px,
};
use hub::api::{Actor, Issue, IssueEventKind, IssuePatch, Run, RunId};
use hub::model::IssueKey;
use hub::{CatalogModel, InboxModel, IssuesModel, LoadState, RunsModel};
use ui::dev::{Probe, Probed};
use ui::prelude::*;
use ui_rich::markdown::Markdown;
use ui_rich::{Composer, TextInput};

/// Width the panel is laid out at until its frame is measured, px (`issue-board.html` draws
/// it 400 wide). The panel fills whatever frame it is given.
pub const PANEL_WIDTH: f32 = 400.;
/// Side padding of a section (`.psec`), px.
const PAD_X: f32 = SPACE_5;
/// Narrowest a section's content gets, px, however narrow the frame.
const MIN_CONTENT_WIDTH: f32 = 240.;
/// How often relative times ("12m") are redrawn.
const TICK: Duration = Duration::from_secs(30);

/// What the panel asks of the view that holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelEvent {
    /// The person closed the panel.
    Close,
}

/// The issue when the board's model does not have it (another project, or just deleted).
enum Fallback {
    Unasked,
    Asking(#[allow(dead_code)] Task<()>),
    Found(Arc<Issue>),
    Missing,
    Failed(SharedString),
}

/// What the panel can show right now.
pub(crate) enum Shown {
    Loading,
    Failed(SharedString),
    Missing,
    Ready(Arc<Issue>),
}

/// Every run of the issue, read when the latest run or the run count changes.
#[derive(Default)]
struct History {
    stamp: Option<(Option<RunId>, u32)>,
    runs: Vec<Arc<Run>>,
    _task: Option<Task<()>>,
}

/// The side panel of one issue. Self-contained: it reads the issue, its runs and its activity
/// from `hub` and acts through `hub::commands`.
pub struct IssuePanel {
    issue: IssueKey,
    issues: Entity<IssuesModel>,
    runs: Entity<RunsModel>,
    inbox: Entity<InboxModel>,
    catalog: Entity<CatalogModel>,
    composer: Entity<Composer>,
    /// The title field while the title is edited in place.
    title: Entity<TextInput>,
    editing_title: bool,
    /// The runner picker's search field and its highlighted row.
    search: Entity<TextInput>,
    picked: usize,
    /// The answer to the question the latest run waits on.
    answer: Entity<TextInput>,
    /// How many dollars more a run stopped at its budget may spend.
    budget: Entity<TextInput>,
    description: Entity<Markdown>,
    /// Comment bodies by event `seq`.
    bodies: HashMap<u64, Entity<Markdown>>,
    fallback: Fallback,
    history: History,
    /// Runs whose steps are shown the other way from the default (the latest unfolded, the
    /// earlier ones folded).
    toggled: HashSet<RunId>,
    input: comment::InputState,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The frame's width as of the last paint: wrapping text is laid out at a definite width.
    width: Rc<Cell<f32>>,
    _subs: Vec<Subscription>,
    _ticker: Task<()>,
}

impl IssuePanel {
    /// The panel of `issue`. Loading starts at once.
    pub fn new(issue: IssueKey, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let issues = hub::issues(cx);
        let runs = hub::runs(cx);
        let inbox = hub::inbox(cx);
        let catalog = hub::catalog(cx);
        issues.update(cx, |model, cx| model.ensure_activity(&issue, cx));
        // The kit draws the placeholder on one line and clips it; the tail goes in the hint,
        // which ends in an ellipsis instead of being cut mid-word in a narrow panel.
        let composer = cx.new(|cx| {
            Composer::issue(cx)
                .name("issue")
                .placeholder("Comment")
                .placeholder_hint(Some("· @mention an agent or pipeline to wake it up".into()))
        });
        let title = cx.new(TextInput::new);
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Find an orchestrator, agent or pipeline"));
        let answer = cx.new(|cx| TextInput::new(cx).placeholder("Answer…"));
        let budget = cx.new(|cx| TextInput::new(cx).with_text("1.00").placeholder("1.00"));
        let description = cx.new(|cx| Markdown::new("", cx).color(TEXT_SECONDARY).on_link(|url, _, cx| cx.open_url(url)));
        let subs = vec![
            cx.observe(&issues, |this, _, cx| this.sync(cx)),
            cx.observe(&runs, |this, _, cx| this.sync(cx)),
            cx.observe(&inbox, |_, _, cx| cx.notify()),
            cx.observe(&catalog, |this, _, cx| this.sync_mentions(cx)),
            cx.subscribe_in(&composer, window, Self::on_composer_event),
            cx.observe(&composer, |_, _, cx| cx.notify()),
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.subscribe(&answer, |this, _, event: &ui_rich::TextInputEvent, cx| {
                if *event == ui_rich::TextInputEvent::Submit {
                    this.send_answer(cx);
                }
            }),
            cx.subscribe_in(&budget, window, |this, _, event: &ui_rich::TextInputEvent, window, cx| {
                if *event == ui_rich::TextInputEvent::Submit {
                    this.raise_budget(window, cx);
                }
            }),
        ];
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            issue,
            issues,
            runs,
            inbox,
            catalog,
            composer,
            title,
            editing_title: false,
            search,
            picked: 0,
            answer,
            budget,
            description,
            bodies: HashMap::new(),
            fallback: Fallback::Unasked,
            history: History::default(),
            toggled: HashSet::new(),
            input: comment::InputState::default(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            width: Rc::new(Cell::new(PANEL_WIDTH)),
            _subs: subs,
            _ticker: ticker,
        };
        this.restore_draft(cx);
        this.sync(cx);
        this.sync_mentions(cx);
        this
    }

    /// The issue it shows.
    pub fn issue(&self) -> &IssueKey {
        &self.issue
    }

    /// Gives the comment box keyboard focus. The panel never takes focus by itself (the board
    /// keeps its arrow keys); the board or the app calls this when the person asks to comment.
    pub fn focus_comment(&self, window: &mut Window, cx: &App) {
        self.composer.read(cx).focus(window, cx);
    }

    /// Width of a section's content, px: what wrapping text inside a section is laid out at.
    pub(crate) fn content_width(&self) -> f32 {
        (self.width.get() - 2. * PAD_X).max(MIN_CONTENT_WIDTH)
    }

    /// The comment box, for the board's keyboard handling and for tests.
    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// What the panel shows now: the issue, or why it cannot.
    pub(crate) fn shown(&self, cx: &App) -> Shown {
        let model = self.issues.read(cx);
        if let Some(issue) = model.issue(&self.issue) {
            return Shown::Ready(issue.clone());
        }
        match (&self.fallback, model.state()) {
            (Fallback::Found(issue), _) => Shown::Ready(issue.clone()),
            (Fallback::Missing, _) => Shown::Missing,
            (Fallback::Failed(why), _) => Shown::Failed(why.clone()),
            (_, LoadState::Failed(why)) => Shown::Failed(why.clone()),
            _ => Shown::Loading,
        }
    }

    /// The issue, when there is one to show.
    pub(crate) fn current(&self, cx: &App) -> Option<Arc<Issue>> {
        match self.shown(cx) {
            Shown::Ready(issue) => Some(issue),
            _ => None,
        }
    }

    /// The latest run of the issue.
    pub(crate) fn latest_run(&self, cx: &App) -> Option<Arc<Run>> {
        if let Some(run) = self.runs.read(cx).latest_for(&self.issue) {
            return Some(run.clone());
        }
        self.history.runs.iter().max_by_key(|run| (run.number, run.started)).cloned()
    }

    /// The runs before the latest one, newest first.
    pub(crate) fn earlier_runs(&self, latest: Option<&RunId>) -> Vec<Arc<Run>> {
        let mut runs: Vec<Arc<Run>> =
            self.history.runs.iter().filter(|run| Some(&run.id) != latest).cloned().collect();
        runs.sort_by_key(|run| std::cmp::Reverse((run.number, run.started)));
        runs
    }

    /// Changes fields of the issue.
    pub(crate) fn patch(&mut self, patch: IssuePatch, cx: &mut Context<Self>) {
        hub::commands(cx).update_issue(&self.issue, patch, cx).detach();
    }

    /// The person closed the panel.
    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Close);
    }

    /// Unfolds or folds the steps of a run.
    pub(crate) fn toggle_run(&mut self, id: &RunId, cx: &mut Context<Self>) {
        if !self.toggled.remove(id) {
            self.toggled.insert(id.clone());
        }
        cx.notify();
    }

    /// Catches up with the models: the issue (or the lookup outside the board), its
    /// description, the comment bodies and the run history.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.issues.update(cx, |model, cx| model.ensure_activity(&self.issue, cx));
        let (ready, present) = {
            let model = self.issues.read(cx);
            (*model.state() == LoadState::Ready, model.issue(&self.issue).is_some())
        };
        if present {
            if !matches!(self.fallback, Fallback::Unasked) {
                self.fallback = Fallback::Unasked;
            }
        } else if ready && matches!(self.fallback, Fallback::Unasked) {
            self.look_up(cx);
        }
        if let Some(issue) = self.current(cx) {
            let body = issue.body.clone();
            self.description.update(cx, |md, cx| md.set_text(body, cx));
            self.sync_history(&issue, cx);
        }
        self.sync_bodies(cx);
        cx.notify();
    }

    /// Reads the issue outside the board's model: it may belong to another project.
    fn look_up(&mut self, cx: &mut Context<Self>) {
        let task = hub::queries(cx).issue(&self.issue, cx);
        self.fallback = Fallback::Asking(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.fallback = match result {
                    Ok(Some(issue)) => Fallback::Found(Arc::new(issue)),
                    Ok(None) => Fallback::Missing,
                    Err(error) => Fallback::Failed(format!("{error:#}").into()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn sync_history(&mut self, issue: &Issue, cx: &mut Context<Self>) {
        let latest = self.runs.read(cx).latest_for(&self.issue).map(|run| run.id.clone()).or_else(|| issue.run.clone());
        let stamp = (latest, issue.run_count);
        if self.history.stamp.as_ref() == Some(&stamp) {
            return;
        }
        self.history.stamp = Some(stamp);
        if issue.run_count == 0 && issue.run.is_none() {
            self.history.runs.clear();
            return;
        }
        let task = hub::queries(cx).runs_for_issue(&self.issue, cx);
        self.history._task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Ok(runs) = result {
                    this.history.runs = runs.into_iter().map(Arc::new).collect();
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// A Markdown view for every comment of the feed, made once per comment.
    fn sync_bodies(&mut self, cx: &mut Context<Self>) {
        let comments: Vec<(u64, String, bool)> = {
            let model = self.issues.read(cx);
            let Some(feed) = model.activity(&self.issue).and_then(|feed| feed.value()) else { return };
            feed.iter()
                .filter_map(|event| match &event.kind {
                    IssueEventKind::Comment { text, .. } => Some((event.seq, text.clone(), event.actor == Actor::Me)),
                    _ => None,
                })
                .collect()
        };
        let names = self.mention_names(cx);
        for (seq, text, mine) in comments {
            if let Some(body) = self.bodies.get(&seq) {
                body.update(cx, |md, cx| md.set_text(text, cx));
                continue;
            }
            // A person's words in `text`, an agent's in `text-secondary`, as the mockup draws them.
            let color = if mine { TEXT } else { TEXT_SECONDARY };
            let names = names.clone();
            let body =
                cx.new(|cx| Markdown::new(text, cx).color(color).mentions(names).on_link(|url, _, cx| cx.open_url(url)));
            self.bodies.insert(seq, body);
        }
    }

    /// Names drawn as mentions in comments: every runnable of the catalog.
    fn mention_names(&self, cx: &App) -> Vec<String> {
        self.catalog.read(cx).runnables().iter().map(|runnable| runnable.name.clone()).collect()
    }

    fn sync_mentions(&mut self, cx: &mut Context<Self>) {
        let names = self.mention_names(cx);
        for body in self.bodies.values() {
            body.update(cx, |md, cx| md.set_mentions(names.clone(), cx));
        }
        cx.notify();
    }

    fn on_cancel(&mut self, _: &keys::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_title || self.composer.read(cx).is_focused(window, cx) {
            cx.propagate();
            return;
        }
        self.close(cx);
    }

    fn body(&mut self, issue: &Arc<Issue>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // A definite width all the way down: text measured inside a shrink-to-fit or row
        // container wraps at the wrong width in this GPUI.
        let mut sections = col().flex_none().w(px(self.width.get()));
        sections = sections.child(props::title_section(self, issue, window, cx));
        sections = sections.child(props::props_section(self, issue, window, cx));
        if let Some(run) = run::section(self, issue, window, cx) {
            sections = sections.child(run);
        }
        sections = sections.child(activity::feed(self, issue, window, cx));
        div()
            .id("issue-panel-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(sections)
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for IssuePanel {}

impl Focusable for IssuePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for IssuePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.shown(cx);
        let width = self.width.clone();
        // Measures the frame; a new width is drawn again at once, so text wraps to it.
        let measure = canvas(
            move |bounds, window, _| {
                let w = f32::from(bounds.size.width);
                if w > 0. && (w - width.get()).abs() > 0.5 {
                    width.set(w);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        // The content is laid out at the measured width, so every `w_full` row and every
        // wrapping text gets the same definite width whatever kind of frame holds the panel.
        let mut panel = col()
            .absolute()
            .top_0()
            .left_0()
            .w(px(self.width.get()))
            .h_full()
            .min_h_0();
        match shown {
            Shown::Ready(issue) => {
                panel = panel
                    .child(header::header(self, Some(&issue), window, cx))
                    .child(self.body(&issue, window, cx))
                    .child(comment::dock(self, cx));
            }
            Shown::Loading => {
                panel = panel.child(header::header(self, None, window, cx)).child(placeholder("Loading the issue…", None, self.content_width()));
            }
            Shown::Missing => {
                panel = panel.child(header::header(self, None, window, cx)).child(placeholder(
                    "Issue not found",
                    Some(format!("{} was deleted or never existed.", self.issue).into()),
                    self.content_width(),
                ));
            }
            Shown::Failed(why) => {
                panel = panel
                    .child(header::header(self, None, window, cx))
                    .child(placeholder("Could not load the issue", Some(why), self.content_width()));
            }
        }
        div()
            .id("issue-panel")
            .key_context("IssuePanel")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_cancel))
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(BG)
            .text_color(TEXT)
            .type_style(BODY)
            .child(measure)
            .child(panel)
            .named("issue-panel")
    }
}

/// The body of a panel with no issue to show: a title and an optional reason.
fn placeholder(title: &'static str, detail: Option<SharedString>, width: f32) -> impl IntoElement {
    col()
        .flex_1()
        .min_h_0()
        .px(px(PAD_X))
        .py(px(SPACE_6))
        .gap(px(SPACE_1))
        .child(div().flex_none().type_style(BODY_STRONG).child(title))
        .when_some(detail, |d, detail| {
            d.child(div().flex_none().w(px(width)).type_style(LABEL).text_color(TEXT_MUTED).child(detail))
        })
        .named("issue-panel-state")
}

/// Names an element for tests (a debug selector) and for the dev driver (a probe).
pub(crate) trait Named: InteractiveElement + IntoElement + Sized {
    /// Record this element under `name` for `debug_bounds` and `harness-dev`.
    fn named(self, name: impl Into<SharedString>) -> Probed<Self> {
        let name = name.into();
        let selector = name.clone();
        self.debug_selector(move || selector.to_string()).probe(name)
    }
}

impl<E: InteractiveElement + IntoElement> Named for E {}

/// A section of the panel (`.psec`): 16/20 padding and a rule under it.
pub(crate) fn section() -> gpui::Div {
    col().flex_none().w_full().px(px(PAD_X)).py(px(SPACE_4)).border_b_1().border_color(LINE)
}

/// The heading row of a section (`.psec-h`): a title and something at the right.
pub(crate) fn section_head(title: impl Into<SharedString>, end: Option<SharedString>) -> impl IntoElement {
    row()
        .flex_none()
        .gap(px(SPACE_2))
        .min_h(px(20.))
        .mb(px(SPACE_2_5))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(div().type_style(BODY_STRONG).text_color(TEXT).child(title.into()))
        .when_some(end, |d, end| d.child(grow()).child(div().flex_none().child(end)))
}
