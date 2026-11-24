//! The Inbox view: its state, what opens when, the header parts and the keyboard.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    MouseButton, Render, SharedString, Subscription, Task, Window, canvas, div, prelude::*, px,
};
use hub::api::{InboxId, InboxItem, InboxKind, PullRequest, ReviewDecision, ReviewId, RunnableRef};
use hub::{
    CatalogModel, InboxFocus, InboxModel, LoadState, ReviewModel, Route, RunModel, RunsModel,
};
use ui::dev::Probe;
use ui::prelude::*;

use crate::list::{self, Tab};
use crate::send_back::SendBack;
use crate::texts::Texts;
use crate::waits::{self, ItemRun};

/// Width of the queue on the left (`aside` in the mockup).
pub(crate) const QUEUE_WIDTH: f32 = 300.;
/// The detail column never gets wider than this (`max-width: 1000px`).
pub(crate) const DETAIL_MAX: f32 = 1000.;
/// Width of the shell's sidebar plus the content frame's margins, for the first frame's guess
/// of the detail width (the real one is measured after layout).
const SHELL_GUESS: f32 = 268.;
/// How often relative times ("40m") are redrawn.
const TICK: Duration = Duration::from_secs(60);

/// A pull request's status, read once per opened item.
pub(crate) enum PrStatus {
    Loading,
    Ready(Box<PullRequest>),
    Failed(SharedString),
}

/// The archived items, read on demand (the live ones are in [`InboxModel`]).
#[derive(Default)]
pub(crate) struct Archived {
    pub(crate) started: bool,
    pub(crate) state: LoadState,
    pub(crate) items: Vec<Arc<InboxItem>>,
    generation: u64,
    task: Option<Task<()>>,
}

/// The review that is open, with the variant and file the person is looking at.
pub(crate) struct OpenReview {
    pub(crate) id: ReviewId,
    pub(crate) model: Entity<ReviewModel>,
    /// The chosen variant (1-based); `None` until the review loaded.
    pub(crate) variant: Option<u32>,
    /// Index of the file of the chosen variant whose diff shows.
    pub(crate) file: usize,
    pub(crate) run: Option<Entity<RunModel>>,
    _subscriptions: Vec<Subscription>,
}

/// The whole content of `Route::Inbox`.
pub struct InboxView {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) focus: Option<InboxFocus>,
    pub(crate) tab: Tab,
    pub(crate) inbox: Entity<InboxModel>,
    catalog: Entity<CatalogModel>,
    runs: Entity<RunsModel>,
    pub(crate) archived: Archived,
    pub(crate) review: Option<OpenReview>,
    /// The selectable texts of the open item or review.
    pub(crate) texts: Texts,
    pub(crate) prs: HashMap<String, PrStatus>,
    pub(crate) send_back: Option<Entity<SendBack>>,
    /// The run of the open item, when it has one (questions, budget stops).
    pub(crate) item_run: Option<ItemRun>,
    /// The item last marked read on opening, so "Mark unread" sticks while it stays open.
    marked: Option<InboxId>,
    /// What the next reconciliation of list and detail does (see `select.rs`).
    pub(crate) pending: crate::select::Pending,
    /// The person picked the tab; an empty Waiting tab then stays shown.
    pub(crate) user_tab: bool,
    /// Index of the open row in the list the last time it was found there.
    pub(crate) row: usize,
    /// Width of the detail column in the last frame.
    pub(crate) detail_width: f32,
    /// A pinned clock for tests; the wall clock otherwise.
    pub(crate) now: Option<i64>,
    /// The queue takes the keys back on the next frame (Esc in a field of the item).
    refocus: bool,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl InboxView {
    /// The Inbox, opened on `focus` when given, else on the first item that waits.
    pub fn new(focus: Option<InboxFocus>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inbox = hub::inbox(cx);
        let catalog = hub::catalog(cx);
        let runs = hub::runs(cx);
        let issues = hub::issues(cx);
        let subscriptions = vec![
            cx.observe(&issues, |_, _, cx| cx.notify()),
            cx.observe(&inbox, |this, _, cx| this.inbox_changed(cx)),
            cx.observe(&catalog, |_, _, cx| cx.notify()),
            cx.observe(&runs, |this, _, cx| {
                this.reconcile(cx);
                cx.notify();
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
        let width = f32::from(window.viewport_size().width) - QUEUE_WIDTH - SHELL_GUESS;
        let mut view = Self {
            focus_handle: cx.focus_handle(),
            focus: None,
            tab: Tab::Waiting,
            inbox,
            catalog,
            runs,
            archived: Archived::default(),
            review: None,
            texts: Texts::default(),
            prs: HashMap::new(),
            send_back: None,
            item_run: None,
            marked: None,
            pending: crate::select::Pending::Follow,
            user_tab: false,
            row: 0,
            detail_width: width.clamp(320., DETAIL_MAX),
            now: None,
            refocus: false,
            _ticker: ticker,
            _subscriptions: subscriptions,
        };
        view.open(focus, false, window, cx);
        window.focus(&view.focus_handle);
        view
    }

    /// Follows the route: opens `focus`.
    pub fn show(&mut self, focus: Option<InboxFocus>, window: &mut Window, cx: &mut Context<Self>) {
        self.user_tab = false;
        if focus.is_some() && focus != self.focus {
            self.open(focus, false, window, cx);
        } else if focus.is_none() && self.focus.is_none() {
            self.reconcile(cx);
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// What is open.
    pub fn focus(&self) -> Option<&InboxFocus> {
        self.focus.as_ref()
    }

    /// What the shell's header shows as the title: "Review" while a review is open, the kind
    /// of the open item otherwise, "Inbox" when nothing is open.
    pub fn title(&self, cx: &App) -> SharedString {
        if self.review.is_some() {
            return "Review".into();
        }
        match self.current_item(cx) {
            Some(item) => list::kind_label(&item).into(),
            None => "Inbox".into(),
        }
    }

    /// The crumbs between "Inbox" and the title: the pipeline of the open review or item
    /// ("PAY triage"), when it is known.
    pub fn trail(&self, cx: &App) -> Vec<SharedString> {
        self.pipeline_name(cx).into_iter().collect()
    }

    /// What follows the title in the shell's header: counts.
    pub fn header_badges(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let waiting = self.live_items(Tab::Waiting, cx).len();
        if waiting == 0 {
            return Vec::new();
        }
        vec![
            div()
                .ml(px(SPACE_2))
                .child(
                    Badge::new(format!("{} waiting", base::fmt::count(waiting as u64)))
                        .tone(BadgeTone::Warning)
                        .dot(Status::Waiting),
                )
                .probe("inbox-waiting")
                .into_any_element(),
        ]
    }

    /// The controls on the right of the shell's header, left to right.
    pub fn header_actions(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut actions = Vec::new();
        if let Some(item) = self.current_item(cx) {
            let (id, read, archived) = (item.id.clone(), item.read, item.archived);
            let read_id = id.clone();
            actions.push(
                Button::new("inbox-read", if read { "Mark unread" } else { "Mark read" })
                    .ghost()
                    .on_click(move |_, _, cx| hub::commands(cx).set_inbox_read(&read_id, !read, cx))
                    .probe("inbox-read")
                    .into_any_element(),
            );
            let this = cx.entity().downgrade();
            actions.push(
                IconButton::new(
                    "inbox-archive",
                    if archived {
                        IconName::Replay
                    } else {
                        IconName::Archive
                    },
                )
                .tooltip(if archived { "Restore" } else { "Archive" })
                .on_click(move |_, window, cx| {
                    this.update(cx, |view, cx| view.archive(&id, !archived, window, cx))
                        .ok();
                })
                .probe("inbox-archive")
                .into_any_element(),
            );
        }
        if let Some(run) = self.review_run(cx) {
            let route = Route::Editor {
                pipeline: run.pipeline.clone(),
                node: self.open_review(cx).map(|r| r.node.clone()),
            };
            actions.push(
                Button::new("inbox-back-to-editor", "Back to editor")
                    .ghost()
                    .icon(IconName::Pipelines)
                    .on_click(move |_, _, cx| hub::navigate(route.clone(), cx))
                    .probe("inbox-back-to-editor")
                    .into_any_element(),
            );
        }
        actions
    }

    // ------------------------------------------------------------ what is open

    /// The current time for relative labels.
    pub(crate) fn now(&self) -> i64 {
        self.now.unwrap_or_else(base::time::now_ms)
    }

    /// One item by id, live or archived.
    pub(crate) fn find_item(&self, id: &InboxId, cx: &App) -> Option<Arc<InboxItem>> {
        self.inbox.read(cx).item(id).cloned().or_else(|| {
            self.archived
                .items
                .iter()
                .find(|item| &item.id == id)
                .cloned()
        })
    }

    /// The item of a review, live or archived.
    fn item_of_review(&self, review: &ReviewId, cx: &App) -> Option<Arc<InboxItem>> {
        let is_it = |item: &&Arc<InboxItem>| matches!(&item.kind, InboxKind::Review { review: r } if r == review);
        self.inbox
            .read(cx)
            .items()
            .iter()
            .find(is_it)
            .cloned()
            .or_else(|| self.archived.items.iter().find(is_it).cloned())
    }

    /// The open item, when the focus names one that is known.
    pub(crate) fn current_item(&self, cx: &App) -> Option<Arc<InboxItem>> {
        match self.focus.as_ref()? {
            InboxFocus::Item(id) => self.find_item(id, cx),
            InboxFocus::Review(review) => self.item_of_review(review, cx),
        }
    }

    /// Whether `item` is the open one.
    pub(crate) fn is_open(&self, item: &InboxItem) -> bool {
        match &self.focus {
            Some(InboxFocus::Item(id)) => &item.id == id,
            Some(InboxFocus::Review(review)) => {
                matches!(&item.kind, InboxKind::Review { review: r } if r == review)
            }
            None => false,
        }
    }

    /// The review the focus leads to, when there is one.
    fn review_for_focus(&self, cx: &App) -> Option<ReviewId> {
        match self.focus.as_ref()? {
            InboxFocus::Review(review) => Some(review.clone()),
            InboxFocus::Item(id) => match &self.find_item(id, cx)?.kind {
                InboxKind::Review { review } => Some(review.clone()),
                _ => None,
            },
        }
    }

    /// The open review, once loaded.
    pub(crate) fn open_review(&self, cx: &App) -> Option<Arc<hub::api::Review>> {
        self.review.as_ref()?.model.read(cx).review().cloned()
    }

    /// The run of the open review, once loaded.
    fn review_run(&self, cx: &App) -> Option<Arc<hub::api::Run>> {
        self.review.as_ref()?.run.as_ref()?.read(cx).run().cloned()
    }

    /// The name of the pipeline the open review or item came through.
    pub(crate) fn pipeline_name(&self, cx: &App) -> Option<SharedString> {
        let pipeline = match self.review_run(cx) {
            Some(run) => run.pipeline.clone(),
            None => {
                let run = self.current_item(cx)?.run.clone()?;
                self.runs.read(cx).find(&run)?.pipeline.clone()
            }
        };
        let name = self
            .catalog
            .read(cx)
            .runnable(&RunnableRef::pipeline(pipeline.as_str()))?
            .name
            .clone();
        Some(name.into())
    }

    /// The name of the pipeline a run went through, when the runs model knows the run.
    pub(crate) fn run_pipeline_name(
        &self,
        run: &hub::api::RunId,
        cx: &App,
    ) -> Option<SharedString> {
        let pipeline = self.runs.read(cx).find(run)?.pipeline.clone();
        let name = self
            .catalog
            .read(cx)
            .runnable(&RunnableRef::pipeline(pipeline.as_str()))?
            .name
            .clone();
        Some(name.into())
    }

    // ------------------------------------------------------------ opening

    /// Opens `focus`: the item, its review, its summary and its pull request. With `route`
    /// the router follows (a selection inside the screen replaces the current route).
    pub(crate) fn open(
        &mut self,
        focus: Option<InboxFocus>,
        route: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A text or field of the item left behind had the keys (a press on a text selects):
        // the queue takes them back, as the item's texts and fields go with it.
        let had_keys = self.texts.focused(window, cx) || self.typing(window, cx);
        self.focus = focus.clone();
        if route {
            let route = Route::Inbox {
                item: focus.clone(),
            };
            hub::router(cx).update(cx, |router, cx| router.replace(route, cx));
        }
        // A click opens a row of the list; a route may name an item of another tab.
        self.pending = if route {
            crate::select::Pending::Keep
        } else {
            crate::select::Pending::Follow
        };
        self.sync_review(cx);
        self.sync_item(cx);
        self.reconcile(cx);
        if had_keys {
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    /// Opens the item at `index` of the visible list.
    pub(crate) fn open_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.visible_items(cx);
        if let Some(item) = items.get(index) {
            self.open(Some(InboxFocus::Item(item.id.clone())), true, window, cx);
        }
    }

    /// Opens the review model the focus leads to, or closes the one open.
    pub(crate) fn sync_review(&mut self, cx: &mut Context<Self>) {
        let wanted = self.review_for_focus(cx);
        if self.review.as_ref().map(|r| &r.id) == wanted.as_ref() {
            return;
        }
        self.review = wanted.map(|id| {
            let model = hub::review(&id, cx);
            let subscriptions = vec![cx.observe(&model, |this, _, cx| this.review_changed(cx))];
            OpenReview {
                id,
                model,
                variant: None,
                file: 0,
                run: None,
                _subscriptions: subscriptions,
            }
        });
        self.review_changed(cx);
    }

    /// The open review loaded or changed: pick a variant, ask for the diff, open the run.
    pub(crate) fn review_changed(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.review.as_mut() else {
            return;
        };
        let Some(review) = open.model.read(cx).review().cloned() else {
            cx.notify();
            return;
        };
        let known = open
            .variant
            .is_some_and(|v| review.variants.iter().any(|variant| variant.index == v));
        if !known {
            open.variant = review
                .picked
                .or_else(|| review.variants.first().map(|v| v.index));
            open.file = 0;
        }
        if open.run.is_none() {
            let run = hub::run(&review.run, cx);
            let subscription = cx.observe(&run, |_, _, cx| cx.notify());
            open._subscriptions.push(subscription);
            open.run = Some(run);
        }
        self.ensure_diff(cx);
        cx.notify();
    }

    /// Asks for the diff of the chosen file of the chosen variant.
    pub(crate) fn ensure_diff(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.review.as_ref() else {
            return;
        };
        let Some(variant) = open.variant else { return };
        let Some(review) = open.model.read(cx).review().cloned() else {
            return;
        };
        let Some(file) = review
            .variants
            .iter()
            .find(|v| v.index == variant)
            .and_then(|v| v.files.get(open.file))
        else {
            return;
        };
        let path = file.path.clone();
        open.model
            .update(cx, |model, cx| model.ensure_diff(variant, &path, cx));
    }

    /// Marks the open item read, renders its summary and asks for its pull request.
    pub(crate) fn sync_item(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.current_item(cx) else {
            return;
        };
        self.sync_item_run(&item, cx);
        if self.marked.as_ref() != Some(&item.id) {
            self.marked = Some(item.id.clone());
            if !item.read && !item.archived {
                hub::commands(cx).set_inbox_read(&item.id, true, cx);
            }
        }
        for link in item
            .links
            .iter()
            .filter(|link| list::is_pull_request(&link.url))
        {
            if self.prs.contains_key(&link.url) {
                continue;
            }
            self.prs.insert(link.url.clone(), PrStatus::Loading);
            let url = link.url.clone();
            let task = hub::queries(cx).pull_request(&url, cx);
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    let status = match result {
                        Ok(pr) => PrStatus::Ready(Box::new(pr)),
                        Err(error) => PrStatus::Failed(format!("{error:#}").into()),
                    };
                    this.prs.insert(url, status);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The inbox model changed: keep the selection, the review and the archive current.
    fn inbox_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_review(cx);
        self.sync_item(cx);
        self.reconcile(cx);
        if self.archived.started {
            self.load_archived(cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ list

    /// The rows of the current tab, in order.
    pub(crate) fn visible_items(&self, cx: &App) -> Vec<Arc<InboxItem>> {
        match self.tab {
            Tab::Archived => self.archived.items.clone(),
            tab => self.live_items(tab, cx),
        }
    }

    /// The live items of a tab, waiting ones first; an item whose run moved on no longer waits.
    pub(crate) fn live_items(&self, tab: Tab, cx: &App) -> Vec<Arc<InboxItem>> {
        match tab {
            Tab::Archived => Vec::new(),
            tab => {
                let inbox = self.inbox.read(cx);
                let runs = self.runs.read(cx);
                let moved_on = |item: &InboxItem| {
                    waits::moved_on(
                        item,
                        item.run.as_ref().and_then(|id| runs.find(id)).map(|r| &**r),
                    )
                };
                list::ordered(inbox.waiting_items(), inbox.items(), tab, moved_on)
            }
        }
    }

    /// Shows another tab; the archive is read when first shown.
    pub(crate) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.user_tab = true;
        self.pending = crate::select::Pending::First;
        if tab == Tab::Archived {
            self.load_archived(cx);
        }
        self.reconcile(cx);
        cx.notify();
    }

    /// Reads the archived items; an older answer is dropped.
    pub(crate) fn load_archived(&mut self, cx: &mut Context<Self>) {
        self.archived.started = true;
        self.archived.generation += 1;
        let generation = self.archived.generation;
        let task = hub::queries(cx).archived_inbox(cx);
        self.archived.task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.archived.generation != generation {
                    return;
                }
                match result {
                    Ok(items) => {
                        this.archived.items = items.into_iter().map(Arc::new).collect();
                        this.archived.state = LoadState::Ready;
                        this.reconcile(cx);
                    }
                    Err(error) => {
                        this.archived.state = LoadState::Failed(format!("{error:#}").into())
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Archives (or restores) an item and opens the next one of the list.
    pub(crate) fn archive(
        &mut self,
        id: &InboxId,
        archived: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.visible_items(cx);
        let next = items
            .iter()
            .position(|item| &item.id == id)
            .and_then(|ix| {
                items
                    .get(ix + 1)
                    .or_else(|| ix.checked_sub(1).and_then(|prev| items.get(prev)))
            })
            .map(|item| item.id.clone());
        hub::commands(cx).archive_inbox(id, archived, cx);
        if self.archived.started {
            self.load_archived(cx);
        }
        if self.current_item(cx).is_some_and(|item| &item.id == id) {
            self.open(next.map(InboxFocus::Item), true, window, cx);
        }
    }

    /// Moves the selection `delta` rows and opens that row.
    pub(crate) fn move_selection(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.visible_items(cx);
        if items.is_empty() {
            return;
        }
        let at = items.iter().position(|item| self.is_open(item));
        let next = match at {
            Some(at) => (at as isize + delta).clamp(0, items.len() as isize - 1) as usize,
            None => 0,
        };
        if Some(next) != at {
            self.open_index(next, window, cx);
        }
    }

    // ------------------------------------------------------------ review actions

    /// Chooses variant `index` of the open review.
    pub(crate) fn pick_variant(&mut self, index: u32, cx: &mut Context<Self>) {
        let Some(review) = self.open_review(cx) else {
            return;
        };
        if !review.variants.iter().any(|v| v.index == index) {
            return;
        }
        if let Some(open) = self.review.as_mut()
            && open.variant != Some(index)
        {
            open.variant = Some(index);
            open.file = 0;
        }
        self.ensure_diff(cx);
        cx.notify();
    }

    /// Shows file `file` of variant `index`.
    pub(crate) fn pick_file(&mut self, index: u32, file: usize, cx: &mut Context<Self>) {
        self.pick_variant(index, cx);
        if let Some(open) = self.review.as_mut() {
            open.file = file;
        }
        self.ensure_diff(cx);
        cx.notify();
    }

    /// Records a decision on the open review.
    pub(crate) fn decide(&mut self, decision: ReviewDecision, cx: &mut Context<Self>) {
        let Some(review) = self.open_review(cx) else {
            return;
        };
        if review.state != hub::api::ReviewState::Open {
            return;
        }
        hub::commands(cx).decide_review(&review.id, decision, cx);
    }

    /// Opens the send-back dialog for the open review.
    pub(crate) fn open_send_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.open_review(cx) else {
            return;
        };
        if review.state != hub::api::ReviewState::Open {
            return;
        }
        let pick = self.review.as_ref().and_then(|r| r.variant).unwrap_or(1);
        let dialog = cx.new(|cx| SendBack::new(review, pick, window, cx));
        self.send_back = Some(dialog.clone());
        let build = dialog.clone();
        overlays(window, cx).open_dialog(
            crate::send_back::DIALOG_ID,
            move |window, cx| {
                build
                    .update(cx, |d, cx| d.dialog(window, cx))
                    .into_any_element()
            },
            window,
            cx,
        );
    }

    /// Gives the keys back to the queue on the next frame.
    pub(crate) fn take_keys(&mut self, cx: &mut Context<Self>) {
        self.refocus = true;
        cx.notify();
    }

    /// Follows the detail column's width; a new width lays the column out again.
    fn fit_detail(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(0., DETAIL_MAX);
        if (width - self.detail_width).abs() >= 0.5 {
            self.detail_width = width;
            cx.notify();
        }
    }

    /// The detail column's content width (its padding taken off).
    pub(crate) fn content_width(&self) -> f32 {
        (self.detail_width - 2. * SPACE_6).max(240.)
    }
}

impl Focusable for InboxView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InboxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus_handle);
        }
        let queue = self.render_queue(window, cx);
        let detail = self.render_detail(window, cx);
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let width = f32::from(bounds.size.width).clamp(0., DETAIL_MAX);
                let Some(view) = this.upgrade() else { return };
                if (width - view.read(cx).detail_width).abs() < 0.5 {
                    return;
                }
                // A notification while the window draws does not draw it again: the new
                // width is taken once this frame is done, and the column laid out anew.
                cx.defer(move |cx| view.update(cx, |view, cx| view.fit_detail(width, cx)));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id("inbox")
            .key_context("Inbox")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // A text field the press focused keeps it, and so does a text the press
                    // selects in (Cmd/Ctrl+C then copies; the queue's keys still bubble up
                    // to it); anything else gives the queue its keys.
                    if !this.typing(window, cx) && !this.texts.focused(window, cx) {
                        window.focus(&this.focus_handle);
                    }
                }),
            )
            .flex()
            .flex_row()
            .size_full()
            .min_h_0()
            .child(queue)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(measure)
                    .child(detail),
            )
    }
}
