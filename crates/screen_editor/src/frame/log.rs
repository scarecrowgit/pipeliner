//! The Log tab: the log of the run on show, one line per row in a virtualised list that
//! follows the tail while it is scrolled to the bottom. Earlier lines than the model keeps
//! are read page by page on demand. With a node selected on the canvas the log can show only
//! that node's lines.
//!
//! The text of a line wraps and can be selected and copied. Its view is made when the row is
//! first drawn and kept by the line's number while the row is drawn; views of rows long out
//! of sight are dropped, so a long log costs what is on screen. All line views share one
//! selection ([`SelectionGroup`], keyed by line number): a drag over several lines selects
//! them all and a copy gives them one per line, rows out of sight included.
//!
//! Probes: `test-run-log` (the list), `log-earlier`, `log-only-node`; debug selectors
//! `log-text-<n>` on the text of the n-th row as listed.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, ListAlignment, ListState, SharedString, Task, Window, div, list, prelude::*, px,
};
use hub::RunLogEvent;
use hub::model::{LogLevel, NodeId, RunLogLine};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::markdown::{Markdown, SelectionGroup};

use super::EditorView;
use crate::Selection;

/// Height of a one-line log row.
const LINE_HEIGHT: f32 = 22.0;
/// Lines read per "Show earlier lines".
const PAGE: usize = 500;
/// Width of the time column and of the node column.
const TIME_WIDTH: f32 = 44.0;
const NODE_WIDTH: f32 = 120.0;
/// How far past the visible rows the list draws, px.
const OVERDRAW: f32 = 200.0;
/// Text views kept before the ones of rows not drawn lately are dropped.
const KEEP_TEXTS: usize = 240;
/// The longest node name in the "Only …" filter chip.
const ONLY_NAME_CHARS: usize = 24;

/// The Log tab's own state: the list, the earlier pages read, the filter, the text views.
pub(super) struct LogState {
    /// Bottom-aligned: at the end it follows new lines, scrolled up it stays put.
    pub(super) list: ListState,
    /// The numbers (`seq`) of the lines the list was last told about, in its order.
    rows: Vec<u64>,
    /// Lines older than the model's first, oldest first.
    pub(super) earlier: Vec<RunLogLine>,
    loading: Option<Task<()>>,
    /// Show only the lines of the node selected on the canvas.
    pub(super) only_node: bool,
    /// Text views by line number, and the frame each was last drawn in.
    texts: HashMap<u64, (Entity<Markdown>, u64)>,
    frame: u64,
    /// One selection across the lines' views.
    group: SelectionGroup,
    /// The rows the group was last given texts for: first and last line number and count.
    group_rows: Option<(u64, u64, usize)>,
}

impl Default for LogState {
    fn default() -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Bottom, px(OVERDRAW)),
            rows: Vec::new(),
            earlier: Vec::new(),
            loading: None,
            only_node: false,
            texts: HashMap::new(),
            frame: 0,
            group: SelectionGroup::new(),
            group_rows: None,
        }
    }
}

impl LogState {
    /// Tells the list how its rows changed from `rows` to `now`: lines dropped at the front,
    /// earlier pages put before, new lines after. Anything else lays the list out anew.
    fn sync(&mut self, now: Vec<u64>) {
        if now == self.rows {
            return;
        }
        let old = &self.rows;
        let gone = now.first().map_or(old.len(), |first| old.iter().take_while(|seq| *seq < first).count());
        let kept = &old[gone..];
        let at = kept.first().and_then(|first| now.binary_search(first).ok());
        match at {
            Some(at) if now.get(at..at + kept.len()) == Some(kept) => {
                // One splice per run of changed rows: one over the anchor resets the scroll.
                if gone > 0 || at > 0 {
                    self.list.splice(0..gone, at);
                }
                let end = at + kept.len();
                if now.len() > end {
                    self.list.splice(end..end, now.len() - end);
                }
            }
            _ => self.list.reset(now.len()),
        }
        self.rows = now;
    }

    /// Drops the text views of rows not drawn lately once there are many.
    fn forget_texts(&mut self, cx: &App) {
        self.frame += 1;
        if self.texts.len() <= KEEP_TEXTS {
            return;
        }
        let recent = self.frame.saturating_sub(2);
        self.texts.retain(|_, (view, drawn)| *drawn >= recent || view.read(cx).selected_text().is_some());
    }
}

impl EditorView {
    /// The node whose lines the log shows, when filtered.
    fn log_node(&self, cx: &App) -> Option<NodeId> {
        if !self.test.log.only_node {
            return None;
        }
        match self.parts.draft.read(cx).selection() {
            Selection::Node(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// The lines the log shows, oldest first.
    pub(crate) fn log_lines<'a>(&'a self, cx: &'a App) -> Vec<&'a RunLogLine> {
        let Some(shown) = &self.test.shown else { return Vec::new() };
        let model = shown.model.read(cx).log();
        let first = model.front().map_or(u64::MAX, |line| line.seq);
        let node = self.log_node(cx);
        self.test
            .log
            .earlier
            .iter()
            .filter(|line| line.seq < first)
            .chain(model.iter())
            .filter(|line| node.is_none() || line.node == node)
            .collect()
    }

    /// The line numbered `seq`, from the earlier pages or the model.
    fn log_line(&self, seq: u64, cx: &App) -> Option<RunLogLine> {
        let shown = self.test.shown.as_ref()?;
        let model = shown.model.read(cx).log();
        if let Ok(ix) = model.binary_search_by_key(&seq, |line| line.seq) {
            return model.get(ix).cloned();
        }
        let earlier = &self.test.log.earlier;
        earlier.binary_search_by_key(&seq, |line| line.seq).ok().and_then(|ix| earlier.get(ix).cloned())
    }

    /// Lines in the log of the run on show, for the tab's count.
    pub(crate) fn log_count(&self, cx: &App) -> usize {
        self.test.shown.as_ref().map_or(0, |shown| shown.model.read(cx).log().len() + self.test.log.earlier.len())
    }

    /// New lines: the list takes them on the next frame and, at the bottom, follows them.
    pub(super) fn log_appended(&mut self, event: &RunLogEvent, cx: &mut Context<Self>) {
        let RunLogEvent::Appended { .. } = event;
        cx.notify();
    }

    /// Reads the page of lines before the first one shown.
    fn load_earlier(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = &self.test.shown else { return };
        let first = self
            .test
            .log
            .earlier
            .first()
            .or_else(|| shown.model.read(cx).log().front())
            .map_or(0, |line| line.seq);
        if first <= 1 || self.test.log.loading.is_some() {
            return;
        }
        let after = first.saturating_sub(PAGE as u64 + 1);
        let asked = hub::queries(cx).run_log(&shown.id, after, PAGE, cx);
        let id = shown.id.clone();
        self.test.log.loading = Some(cx.spawn(async move |this, cx| {
            let page = asked.await;
            this.update(cx, |this, cx| {
                this.test.log.loading = None;
                if this.test.shown.as_ref().is_none_or(|shown| shown.id != id) {
                    return;
                }
                if let Ok(page) = page {
                    let mut lines: Vec<RunLogLine> = page.into_iter().filter(|line| line.seq < first).collect();
                    lines.append(&mut this.test.log.earlier);
                    this.test.log.earlier = lines;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle_only_node(&mut self, cx: &mut Context<Self>) {
        self.test.log.only_node = !self.test.log.only_node;
        cx.notify();
    }

    /// The text view of `line`, made on first use.
    fn log_text(&mut self, line: &RunLogLine, cx: &mut Context<Self>) -> Entity<Markdown> {
        let (color, style) = match line.level {
            LogLevel::Info => (TEXT, LABEL),
            // Mono, on the label's line height so every row keeps its height.
            LogLevel::Tool => (TEXT_SECONDARY, TypeStyle { line: LABEL.line, ..MONO }),
            LogLevel::Warn => (WARNING, LABEL),
            LogLevel::Error => (DANGER, LABEL),
        };
        let frame = self.test.log.frame;
        if let Some((view, drawn)) = self.test.log.texts.get_mut(&line.seq) {
            *drawn = frame;
            let view = view.clone();
            view.update(cx, |view, cx| view.set_text(line.text.clone(), cx));
            return view;
        }
        let group = &self.test.log.group;
        let key = usize::try_from(line.seq).unwrap_or(usize::MAX);
        let view = cx.new(|cx| Markdown::verbatim(line.text.clone(), cx).type_style(style).color(color).group(group, key));
        self.test.log.texts.insert(line.seq, (view.clone(), frame));
        view
    }

    /// Row `ix` of the list: time, node, and the text at `text_width`.
    fn log_row(&mut self, ix: usize, text_width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(line) = self.test.log.rows.get(ix).and_then(|seq| self.log_line(*seq, cx)) else {
            return div().h(px(LINE_HEIGHT)).into_any_element();
        };
        let name = {
            let draft = self.parts.draft.read(cx);
            match (&line.node, draft.graph()) {
                (Some(id), Some(graph)) => graph.node(id).map_or_else(|| id.to_string(), |n| n.name.clone()),
                (Some(id), None) => id.to_string(),
                (None, _) => "run".to_string(),
            }
        };
        let offset = base::time::utc_offset(base::time::now_ms());
        let text = self.log_text(&line, cx);
        row()
            .id(("log-line", ix))
            .w_full()
            .items_start()
            .min_h(px(LINE_HEIGHT))
            .py(px((LINE_HEIGHT - LABEL.line) / 2.))
            .px(px(SPACE_5))
            .gap(px(SPACE_3))
            .child(
                div()
                    .w(px(TIME_WIDTH))
                    .flex_none()
                    .mt(px((LABEL.line - MONO_SM.line) / 2.))
                    .type_style(MONO_SM)
                    .text_color(TEXT_MUTED)
                    .child(base::time::clock_time(line.at, offset)),
            )
            .child(
                div()
                    .w(px(NODE_WIDTH))
                    .flex_none()
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(truncate_lh(name, LABEL.line)),
            )
            .child(div().w(px(text_width)).flex_none().debug_selector(move || format!("log-text-{ix}")).child(text))
            .into_any_element()
    }

    pub(super) fn render_log(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.test.shown.is_none() {
            return col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(SPACE_1))
                .child(div().type_style(BODY_STRONG).child("No test run on show"))
                .child(
                    div()
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .child("Start one from the Test run tab and its log appears here."),
                )
                .probe("test-run-log")
                .into_any_element();
        }
        let lines = self.log_lines(cx);
        let now: Vec<u64> = lines.iter().map(|line| line.seq).collect();
        // The group copies rows that have no view from these texts.
        let shape = now.first().zip(now.last()).map(|(a, b)| (*a, *b, now.len()));
        if shape != self.test.log.group_rows {
            let texts: Vec<(usize, SharedString)> =
                lines.iter().map(|l| (usize::try_from(l.seq).unwrap_or(usize::MAX), SharedString::from(l.text.clone()))).collect();
            self.test.log.group.set_texts(texts);
            self.test.log.group_rows = shape;
        }
        let count = now.len();
        self.test.log.sync(now);
        self.test.log.forget_texts(cx);
        let first = self
            .test
            .log
            .earlier
            .first()
            .or_else(|| self.test.shown.as_ref().and_then(|shown| shown.model.read(cx).log().front()))
            .map_or(0, |line| line.seq);
        let selected = match self.parts.draft.read(cx).selection() {
            Selection::Node(id) => self.parts.draft.read(cx).graph().and_then(|g| g.node(id)).map(|n| n.name.clone()),
            _ => None,
        };
        let bar = row()
            .flex_none()
            .h(px(CONTROL_HEIGHT_SM + SPACE_2 * 2.))
            .px(px(SPACE_5))
            .gap(px(SPACE_2))
            .border_b_1()
            .border_color(LINE_SUBTLE)
            .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(base::fmt::plural(count as u64, "line", "lines")))
            .when(first > 1, |d| {
                d.child(
                    div()
                        .flex_none()
                        .child(
                            Link::new("log-earlier", if self.test.log.loading.is_some() { "Reading…" } else { "Show earlier lines" })
                                .on_click(cx.listener(|this, _, _, cx| this.load_earlier(cx))),
                        )
                        .probe("log-earlier"),
                )
            })
            .child(grow())
            .when_some(selected, |d, name| {
                d.child(
                    div()
                        .flex_none()
                        .child(
                            Chip::new("log-only-node", format!("Only {}", base::fmt::truncate_chars(&name, ONLY_NAME_CHARS)))
                                .icon(IconName::Filter)
                                .pressed(self.test.log.only_node)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_only_node(cx))),
                        )
                        .probe("log-only-node"),
                )
            });
        // A definite width for the wrapping text: the dock less the padding and the columns.
        let text_width = (self.measured.left_width() - 2. * SPACE_5 - TIME_WIDTH - NODE_WIDTH - 2. * SPACE_3).max(0.);
        let list = list(
            self.test.log.list.clone(),
            cx.processor(move |this, ix: usize, _window, cx| this.log_row(ix, text_width, cx)),
        )
        .flex_1()
        .min_h_0()
        .py(px(SPACE_1));
        let body = match self.empty_log_note(count, cx) {
            Some(note) => div()
                .flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .debug_selector(|| "log-empty".into())
                .child(note)
                .into_any_element(),
            None => list.into_any_element(),
        };
        col()
            .flex_1()
            .min_h_0()
            .child(bar)
            .child(div().flex().flex_col().flex_1().min_h_0().child(body).probe("test-run-log"))
            .into_any_element()
    }

    /// What an empty log says instead of a blank list: the filter left nothing, or the run
    /// has not written (or never wrote) a line.
    fn empty_log_note(&self, count: usize, cx: &App) -> Option<String> {
        if count > 0 {
            return None;
        }
        if let Some(node) = self.log_node(cx) {
            let draft = self.parts.draft.read(cx);
            let name = draft.graph().and_then(|g| g.node(&node)).map_or_else(|| node.to_string(), |n| n.name.clone());
            return Some(format!("No lines from {name} in this run"));
        }
        let open = self.shown_run(cx).is_none_or(|run| run.state.is_open());
        Some(if open { "No lines yet: they appear here as the run goes" } else { "This run wrote no log lines" }.to_string())
    }
}
