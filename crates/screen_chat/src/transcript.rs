//! The transcript of one session: messages, thinking, tool calls with their results and
//! diffs, todo lists, sub-agents, permission questions, compaction and error notices.
//!
//! The view is one GPUI `list()` with a row per [`Block`] of the session's model plus a footer
//! row (queued messages, the working line). The model reports every change as a [`Splice`];
//! the view applies it to its `ListState` and re-measures the neighbours whose look depends on
//! the changed rows (tool groups, article headers, a tool row whose permission question came
//! or went). Nothing is rebuilt on a streamed delta.
//!
//! The list is bottom-aligned: while it sits at the end it follows the stream, a scroll up
//! stops that and shows "Jump to latest", and nearing the top loads the older page with the
//! visible rows kept in place.
//!
//! | Module | Draws |
//! | --- | --- |
//! | [`rows`] | the row frame: column width, article spacing, group position |
//! | [`message`] | user messages, agent text, thinking, the article header |
//! | [`tools`] | tool calls: line groups, commands, edits, other tools |
//! | [`permission`] | the permission question on its tool row |
//! | [`agents`] | sub-agent groups and todo lists |
//! | [`notices`] | compaction, notices, failures, the footer, empty and error states |
//! | [`links`] | which permission question belongs on which tool row |
//! | [`texts`] | the selectable texts of tool cards, errors, summaries and questions |

mod agents;
mod links;
mod message;
mod notices;
mod permission;
mod rows;
mod texts;
mod tools;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, IntoElement, ListAlignment, ListOffset, ListScrollEvent, ListState, Render,
    SharedString, Subscription, Task, Window, div, list, prelude::*, px,
};
use hub::model::{PermissionDecision, RequestId, SessionId};
use hub::transcript::{BlockId, BlockKind, ToolState};
use hub::{Block, CatalogModel, LoadState, SessionEvent, SessionModel, Splice};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::diff_view::DiffFile;
use ui_rich::markdown::Markdown;

use links::Links;

/// Rendered height kept around the viewport so scrolling does not pop rows in.
const OVERDRAW: f32 = 600.;
/// Rows from the top at which the older page starts loading.
const LOAD_OLDER_WITHIN: usize = 2;
/// How often running durations refresh.
const TICK: Duration = Duration::from_secs(1);

/// What a person opened or closed on one row, kept across frames and scrolling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RowUi {
    /// Thinking or command output shown (`None`: the default for the row's state).
    pub open: Option<bool>,
    /// "Show more" was clicked.
    pub expanded: bool,
    /// An edit's diff card: folded or not, "Show all" or not. Held here so it survives the
    /// row scrolling out of the list and back.
    pub diff: ui_rich::diff_view::DiffCardState,
}

/// A Reject being written: the request, its one-line note field.
pub(crate) struct Rejecting {
    pub request: RequestId,
    pub input: Entity<ui_rich::TextInput>,
    _sub: Subscription,
}

/// When the view saw a tool call start and finish, for its duration.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    pub start: Instant,
    pub end: Option<Instant>,
}

impl Timing {
    /// Seconds from start to end, or to now while it runs.
    pub fn secs(&self) -> u64 {
        self.end.unwrap_or_else(Instant::now).saturating_duration_since(self.start).as_secs()
    }
}

/// The scrolling transcript of one session. Self-contained: it reads the session's model from
/// `hub`, follows the stream, loads older pages when scrolled up and answers permission
/// questions through `hub::commands`.
pub struct TranscriptView {
    session: SessionId,
    model: Entity<SessionModel>,
    catalog: Entity<CatalogModel>,
    list: ListState,
    links: Links,
    ui: HashMap<BlockId, RowUi>,
    markdown: HashMap<BlockId, Entity<Markdown>>,
    /// Selectable texts that are not messages: tool input and output, errors, summaries.
    texts: HashMap<texts::TextKey, Entity<Markdown>>,
    diffs: HashMap<BlockId, (Arc<Block>, Arc<DiffFile>)>,
    timings: HashMap<String, Timing>,
    /// Each call's display from before its result, by call id: an interrupted call's result
    /// replaces it with a generic error, and the row keeps the look of its kind.
    call_displays: HashMap<String, hub::model::ToolDisplay>,
    rejecting: Option<Rejecting>,
    now: Option<i64>,
    ticker: Option<Task<()>>,
    older_check: bool,
    /// Width of the centred column, from the list's width in the last frame.
    column_width: f32,
    _subscriptions: Vec<Subscription>,
}

impl TranscriptView {
    /// A transcript for `session`. Loading starts at once.
    pub fn new(session: SessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = hub::session(&session, cx);
        let count = model.read(cx).blocks().len() + 1;
        let list = ListState::new(count, ListAlignment::Bottom, px(OVERDRAW));
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |event: &ListScrollEvent, _, cx| {
            // The list state is borrowed while this runs: only the model may be touched.
            let near_top = event.visible_range.start <= LOAD_OLDER_WITHIN;
            if let Some(view) = weak.upgrade() {
                let model = view.read(cx).model.clone();
                if near_top {
                    model.update(cx, |model, cx| model.load_older(cx));
                }
            }
        });
        let catalog = hub::catalog(cx);
        let subscriptions = vec![
            cx.subscribe(&model, |this: &mut Self, _, event: &SessionEvent, cx| this.on_event(event, cx)),
            cx.observe(&model, |this: &mut Self, _, cx| {
                // A question answered elsewhere takes its note field with it.
                let gone = this.rejecting.as_ref().is_some_and(|r| {
                    !this.model.read(cx).pending().iter().any(|request| request.id == r.request)
                });
                if gone {
                    this.cancel_reject(cx);
                }
                this.prune_texts(cx);
                this.remeasure_footer();
                this.ensure_ticker(cx);
                cx.notify();
            }),
            cx.observe(&catalog, |_, _, cx| cx.notify()),
        ];
        let mut view = Self {
            session,
            model,
            catalog,
            list,
            links: Links::default(),
            ui: HashMap::new(),
            markdown: HashMap::new(),
            texts: HashMap::new(),
            diffs: HashMap::new(),
            timings: HashMap::new(),
            call_displays: HashMap::new(),
            rejecting: None,
            now: None,
            ticker: None,
            older_check: false,
            column_width: rows::column_width(f32::from(window.viewport_size().width)),
            _subscriptions: subscriptions,
        };
        view.links = Links::compute(view.model.read(cx).blocks());
        let blocks: Vec<Arc<Block>> = view.model.read(cx).blocks().to_vec();
        for block in &blocks {
            view.remember_display(block);
        }
        view
    }

    /// The session it shows.
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Jumps to the newest message and resumes following the stream.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        let count = self.list.item_count();
        self.list.scroll_to(ListOffset { item_ix: count, offset_in_item: px(0.) });
        cx.notify();
    }

    /// The view sits at the newest row and follows the stream.
    pub fn is_following(&self) -> bool {
        self.list.logical_scroll_top().item_ix >= self.list.item_count()
    }

    /// The session's model, shared with every other view of the session.
    pub fn model(&self) -> &Entity<SessionModel> {
        &self.model
    }

    /// The pending permission question the keyboard answers: the newest one.
    pub fn pending_request(&self, cx: &App) -> Option<RequestId> {
        self.model.read(cx).pending().last().map(|request| request.id.clone())
    }

    /// Answers the newest pending question with "Allow once" (the `↵` hint of the banner).
    pub fn allow_pending(&mut self, cx: &mut Context<Self>) -> bool {
        match self.pending_request(cx) {
            Some(id) => {
                self.answer(&id, PermissionDecision::AllowOnce, cx);
                true
            }
            None => false,
        }
    }

    /// Rejects the newest pending question with an optional note (the `Esc` hint).
    pub fn reject_pending(&mut self, note: &str, cx: &mut Context<Self>) -> bool {
        match self.pending_request(cx) {
            Some(id) => {
                self.answer(&id, PermissionDecision::Reject { note: note.to_string() }, cx);
                true
            }
            None => false,
        }
    }

    /// Opens the note field of Reject on the newest pending question (the `Esc` hint), with
    /// focus in it: Enter rejects with the note, Escape puts the answers back.
    pub fn start_reject(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.pending_request(cx) {
            Some(id) => {
                self.start_reject_for(&id, window, cx);
                true
            }
            None => false,
        }
    }

    /// A Reject note is being written.
    pub fn is_rejecting(&self) -> bool {
        self.rejecting.is_some()
    }

    pub(crate) fn start_reject_for(&mut self, id: &RequestId, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| ui_rich::TextInput::new(cx).placeholder("Tell the agent why, or what to do instead"));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &ui_rich::TextInputEvent, _, cx| match event {
            ui_rich::TextInputEvent::Submit => this.send_reject(cx),
            ui_rich::TextInputEvent::Cancel => this.cancel_reject(cx),
            _ => {}
        });
        input.read(cx).focus(window);
        self.rejecting = Some(Rejecting { request: id.clone(), input, _sub: sub });
        self.remeasure_all();
        cx.notify();
    }

    /// Rejects the question being answered with what the note field holds.
    pub(crate) fn send_reject(&mut self, cx: &mut Context<Self>) {
        let Some(rejecting) = self.rejecting.take() else { return };
        let note = rejecting.input.read(cx).text().trim().to_string();
        self.answer(&rejecting.request, PermissionDecision::Reject { note }, cx);
        self.remeasure_all();
        cx.notify();
    }

    /// Closes the note field; the question keeps its answers.
    pub(crate) fn cancel_reject(&mut self, cx: &mut Context<Self>) {
        if self.rejecting.take().is_some() {
            self.remeasure_all();
            cx.notify();
        }
    }

    /// The note field of a Reject on `id`, if one is open.
    pub(crate) fn reject_note(&self, id: &RequestId) -> Option<&Entity<ui_rich::TextInput>> {
        self.rejecting.as_ref().filter(|r| &r.request == id).map(|r| &r.input)
    }

    /// Fixes the clock used for "2h" times, for previews and tests.
    pub fn set_now(&mut self, now: i64, cx: &mut Context<Self>) {
        self.now = Some(now);
        cx.notify();
    }

    pub(crate) fn answer(&mut self, id: &RequestId, decision: PermissionDecision, cx: &mut Context<Self>) {
        hub::commands(cx).answer_permission(id, decision, cx);
    }

    pub(crate) fn now(&self) -> i64 {
        self.now.unwrap_or_else(base::time::now_ms)
    }

    /// A row's open state, and whether a person changed it.
    pub(crate) fn row_ui(&self, id: BlockId) -> RowUi {
        self.ui.get(&id).copied().unwrap_or_default()
    }

    /// Opens or closes a row's body (thinking, command output).
    pub(crate) fn toggle_open(&mut self, id: BlockId, default_open: bool, cx: &mut Context<Self>) {
        let ui = self.ui.entry(id).or_default();
        ui.open = Some(!ui.open.unwrap_or(default_open));
        cx.notify();
    }

    /// Shows the rest of a clamped row.
    pub(crate) fn expand(&mut self, id: BlockId, cx: &mut Context<Self>) {
        self.ui.entry(id).or_default().expanded = true;
        cx.notify();
    }

    fn on_event(&mut self, event: &SessionEvent, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Spliced(splice) => self.apply_splice(splice, cx),
            // Bottom alignment keeps a following view at the end; a scrolled one stays put.
            SessionEvent::ScrollToBottom | SessionEvent::PermissionArrived(_) => {}
            SessionEvent::TurnFinished(_) | SessionEvent::Failed(_) | SessionEvent::Removed => cx.notify(),
        }
    }

    /// Applies one change of the model's blocks to the list and to the view's caches.
    fn apply_splice(&mut self, splice: &Splice, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let blocks: Vec<Arc<Block>> = model.blocks().to_vec();
        let busy = model.is_busy();
        self.splice_list(splice.old.clone(), splice.count);
        let fresh = splice.old.start..(splice.old.start + splice.count).min(blocks.len());

        let old_links = std::mem::take(&mut self.links);
        self.links = Links::compute(&blocks);
        for index in old_links.moved_rows(splice).symmetric_difference(&self.links.rows()) {
            if !fresh.contains(index) {
                self.remeasure(*index..*index + 1);
            }
        }
        let before = rows::group_start(&blocks, &self.links, fresh.start);
        if before < fresh.start {
            self.remeasure(before..fresh.start);
        }
        let after = rows::group_end(&blocks, &self.links, fresh.end);
        if fresh.end < after {
            self.remeasure(fresh.end..after);
        }
        self.remeasure_footer();
        if self.list.item_count() != blocks.len() + 1 {
            self.list.reset(blocks.len() + 1);
        }

        for block in &blocks[fresh.clone()] {
            self.sync_block(block, busy, cx);
        }
        self.ensure_ticker(cx);
        cx.notify();
    }

    /// Keeps the display of a call that has no result yet.
    fn remember_display(&mut self, block: &Block) {
        if let BlockKind::Tool(tool) = &block.kind
            && !matches!(tool.state, ToolState::Finished { .. })
            && let Some(display) = &tool.display
        {
            self.call_displays.insert(tool.id.clone(), display.clone());
        }
    }

    /// Keeps a row's cached markdown and timing in step with its block.
    fn sync_block(&mut self, block: &Arc<Block>, busy: bool, cx: &mut Context<Self>) {
        self.remember_display(block);
        if let Some(text) = message::markdown_text(&block.kind)
            && let Some(md) = self.markdown.get(&block.id)
        {
            let text = SharedString::from(text.to_string());
            md.update(cx, |md, cx| md.set_text(text, cx));
        }
        if let BlockKind::Tool(tool) = &block.kind {
            match tool.state {
                ToolState::Started | ToolState::Ready if busy => {
                    self.timings.entry(tool.id.clone()).or_insert(Timing { start: Instant::now(), end: None });
                }
                ToolState::Finished { .. } => {
                    if let Some(timing) = self.timings.get_mut(&tool.id) {
                        timing.end.get_or_insert_with(Instant::now);
                    }
                }
                _ => {}
            }
        }
    }

    /// `ListState::splice` that keeps the top visible row in place when it was only edited.
    fn splice_list(&self, old: Range<usize>, count: usize) {
        let top = self.list.logical_scroll_top();
        let following = top.item_ix >= self.list.item_count();
        self.list.splice(old.clone(), count);
        if !following && old.contains(&top.item_ix) && top.item_ix < old.start + count {
            self.list.scroll_to(top);
        }
    }

    /// Drops the cached heights of `range` so the rows are measured again.
    fn remeasure(&self, range: Range<usize>) {
        let range = range.start.min(self.list.item_count())..range.end.min(self.list.item_count());
        if !range.is_empty() {
            self.splice_list(range.clone(), range.len());
        }
    }

    /// Every row measured again (a banner changed its height in place).
    fn remeasure_all(&self) {
        let count = self.list.item_count();
        self.remeasure(0..count);
    }

    fn remeasure_footer(&self) {
        let count = self.list.item_count();
        if count > 0 {
            self.remeasure(count - 1..count);
        }
    }

    /// Refreshes running durations once a second while a tool call runs.
    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        let running = self.model.read(cx).is_busy() && self.timings.values().any(|timing| timing.end.is_none());
        if !running {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let go_on = this.update(cx, |this, cx| {
                    cx.notify();
                    this.model.read(cx).is_busy() && this.timings.values().any(|timing| timing.end.is_none())
                });
                if !matches!(go_on, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.ticker = None);
        }));
    }

    /// Loads the older page when the top rows are in view or every loaded row fits (then no
    /// scroll event ever comes).
    fn check_older(&mut self, cx: &mut Context<Self>) {
        self.older_check = false;
        let model = self.model.read(cx);
        if !model.has_older() || model.is_loading_older() || model.blocks().is_empty() {
            return;
        }
        let viewport = self.list.viewport_bounds().size.height;
        let fits = viewport > px(0.) && self.list.max_offset_for_scrollbar().height <= px(0.);
        let near_top = !self.is_following() && self.list.logical_scroll_top().item_ix <= LOAD_OLDER_WITHIN;
        if fits || near_top {
            self.model.update(cx, |model, cx| model.load_older(cx));
        }
    }

    /// Follows the list's width; a new width measures every row again.
    fn fit_column(&mut self, window: &Window) -> bool {
        let list = f32::from(self.list.viewport_bounds().size.width);
        let available = if list > 0. { list } else { f32::from(window.viewport_size().width) };
        let width = rows::column_width(available);
        if (width - self.column_width).abs() >= 0.5 {
            self.column_width = width;
            let count = self.list.item_count();
            self.splice_list(0..count, count);
            return true;
        }
        false
    }

    fn render_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let blocks: Vec<Arc<Block>> = self.model.read(cx).blocks().to_vec();
        if index >= blocks.len() {
            return self.footer(window, cx).into_any_element();
        }
        rows::row(self, &blocks, index, window, cx)
    }
}

impl Render for TranscriptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let state = model.state().clone();
        let removed = model.is_removed();
        let empty = model.blocks().is_empty() && !model.is_busy() && model.queued().is_empty();
        let has_older = model.has_older();
        let loading_older = model.is_loading_older();
        let root = div().id("transcript").relative().flex().flex_col().size_full().min_h_0();
        if removed {
            return root.child(notices::removed_state()).probe("transcript").into_any_element();
        }
        match state {
            LoadState::Loading => return root.child(notices::loading_state()).probe("transcript").into_any_element(),
            LoadState::Failed(error) => {
                let error = self.verbatim(texts::TextKey::LoadError, &error, LABEL, TEXT_SECONDARY, cx);
                return root.child(notices::error_state(error)).probe("transcript").into_any_element();
            }
            LoadState::Ready if empty => {
                return root.child(notices::empty_state()).probe("transcript").into_any_element();
            }
            LoadState::Ready => {}
        }
        if has_older && !loading_older && !self.older_check {
            self.older_check = true;
            cx.defer_in(window, |this, _, cx| this.check_older(cx));
        }
        self.fit_column(window);
        // The list's own width is known only after this frame's layout.
        cx.defer_in(window, |this, window, cx| {
            if this.fit_column(window) {
                cx.notify();
            }
        });
        let following = self.is_following();
        let rows = list(self.list.clone(), cx.processor(|this, index: usize, window, cx| this.render_row(index, window, cx)))
            .flex_1()
            .size_full();
        let jump = (!following).then(|| {
            row()
                .absolute()
                .bottom(px(SPACE_4))
                .left_0()
                .right_0()
                .justify_center()
                .child(
                    Button::new("jump-latest", "Jump to latest")
                        .small()
                        .icon(IconName::ChevronDown)
                        .on_click(cx.listener(|this, _, _, cx| this.scroll_to_end(cx)))
                        .probe("jump-latest"),
                )
        });
        let older = loading_older.then(|| {
            row()
                .absolute()
                .top(px(SPACE_3))
                .left_0()
                .right_0()
                .justify_center()
                .child(Badge::new("Loading earlier messages").dot(Status::Running))
        });
        root.child(rows).children(older).children(jump).probe("transcript").into_any_element()
    }
}
