//! The Log tab: the log of the run on show, one line per row in a virtualised list that
//! follows the tail while it is scrolled to the bottom. Earlier lines than the model keeps
//! are read page by page on demand. With a node selected on the canvas the log can show only
//! that node's lines.
//!
//! Probes: `test-run-log` (the list), `log-earlier`, `log-only-node`.

use std::ops::Range;

use gpui::{
    AnyElement, App, Context, IntoElement, ScrollStrategy, Task, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};
use hub::RunLogEvent;
use hub::model::{LogLevel, NodeId, RunLogLine};
use ui::dev::Probe;
use ui::prelude::*;

use super::EditorView;
use crate::Selection;

/// Height of a log line.
const LINE_HEIGHT: f32 = 22.0;
/// Lines read per "Show earlier lines".
const PAGE: usize = 500;
/// Width of the time column and of the node column.
const TIME_WIDTH: f32 = 44.0;
const NODE_WIDTH: f32 = 120.0;

/// The Log tab's own state: where the list is scrolled, the earlier pages read, the filter.
pub(super) struct LogState {
    pub(super) scroll: UniformListScrollHandle,
    /// Lines older than the model's first, oldest first.
    pub(super) earlier: Vec<RunLogLine>,
    loading: Option<Task<()>>,
    /// Show only the lines of the node selected on the canvas.
    pub(super) only_node: bool,
    /// Scroll to the last line once there is one: the run was just put on show.
    jump_to_end: bool,
}

impl Default for LogState {
    fn default() -> Self {
        Self { scroll: UniformListScrollHandle::new(), earlier: Vec::new(), loading: None, only_node: false, jump_to_end: true }
    }
}

/// The list is scrolled to its end (or everything fits).
fn at_bottom(scroll: &UniformListScrollHandle) -> bool {
    let state = scroll.0.borrow();
    let offset = -f32::from(state.base_handle.offset().y);
    let max = f32::from(state.base_handle.max_offset().height);
    offset >= max - LINE_HEIGHT
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

    /// Lines in the log of the run on show, for the tab's count.
    pub(crate) fn log_count(&self, cx: &App) -> usize {
        self.test.shown.as_ref().map_or(0, |shown| shown.model.read(cx).log().len() + self.test.log.earlier.len())
    }

    /// New lines: keep the tail in view if it was in view.
    pub(super) fn log_appended(&mut self, event: &RunLogEvent, cx: &mut Context<Self>) {
        let RunLogEvent::Appended { .. } = event;
        if at_bottom(&self.test.log.scroll) {
            let count = self.log_lines(cx).len();
            if count > 0 {
                self.test.log.scroll.scroll_to_item(count - 1, ScrollStrategy::Bottom);
            }
        }
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

    /// Rows `range` of the list.
    fn log_rows(&self, range: Range<usize>, cx: &App) -> Vec<AnyElement> {
        let draft = self.parts.draft.read(cx);
        let name = |node: &Option<NodeId>| -> String {
            match (node, draft.graph()) {
                (Some(id), Some(graph)) => graph.node(id).map_or_else(|| id.to_string(), |n| n.name.clone()),
                (Some(id), None) => id.to_string(),
                (None, _) => "run".to_string(),
            }
        };
        let offset = base::time::utc_offset(base::time::now_ms());
        let lines = self.log_lines(cx);
        range
            .filter_map(|ix| lines.get(ix).map(|line| (ix, *line)))
            .map(|(ix, line)| {
                let (color, mono) = match line.level {
                    LogLevel::Info => (TEXT, false),
                    LogLevel::Tool => (TEXT_SECONDARY, true),
                    LogLevel::Warn => (WARNING, false),
                    LogLevel::Error => (DANGER, false),
                };
                row()
                    .id(("log-line", ix))
                    .h(px(LINE_HEIGHT))
                    .px(px(SPACE_5))
                    .gap(px(SPACE_3))
                    .child(
                        div()
                            .w(px(TIME_WIDTH))
                            .flex_none()
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
                            .child(truncate_lh(name(&line.node), LABEL.line)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .type_style(if mono { MONO } else { LABEL })
                            .text_color(color)
                            .child(truncate_lh(line.text.clone(), LABEL.line)),
                    )
                    .into_any_element()
            })
            .collect()
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
        let count = self.log_lines(cx).len();
        if self.test.log.jump_to_end && count > 0 {
            self.test.log.jump_to_end = false;
            self.test.log.scroll.scroll_to_item(count - 1, ScrollStrategy::Bottom);
        }
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
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(base::fmt::plural(count as u64, "line", "lines")))
            .when(first > 1, |d| {
                d.child(
                    div()
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
                        .child(
                            Chip::new("log-only-node", format!("Only {name}"))
                                .icon(IconName::Filter)
                                .pressed(self.test.log.only_node)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_only_node(cx))),
                        )
                        .probe("log-only-node"),
                )
            });
        let list = uniform_list(
            "test-run-log-list",
            count,
            cx.processor(|this, range: Range<usize>, _window, cx| this.log_rows(range, cx)),
        )
        .track_scroll(self.test.log.scroll.clone())
        .flex_1()
        .min_h_0()
        .py(px(SPACE_1));
        col()
            .flex_1()
            .min_h_0()
            .child(bar)
            .child(div().flex().flex_col().flex_1().min_h_0().child(list).probe("test-run-log"))
            .into_any_element()
    }
}
