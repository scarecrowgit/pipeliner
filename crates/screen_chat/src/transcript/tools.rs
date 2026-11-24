//! Tool calls by how they are shown: short calls (`read`, `grep`) as rows of one shared card,
//! commands with their output, edits and new files as diffs, anything else as a titled text.
//! Each says where it is: preparing, running, done, failed or interrupted, with its duration
//! when the view saw it run.

use std::sync::Arc;

use gpui::{AnyElement, App, ClipboardItem, Context, ElementId, IntoElement, SharedString, div, prelude::*, px};
use hub::Block;
use hub::model::{FileDiff, ToolDisplay};
use hub::transcript::{ToolBlock, ToolState};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::diff_view::{DiffCard, DiffFile, DiffHunk, DiffLine, DiffLineKind};

use super::TranscriptView;
use super::rows::{Place, group_card};

/// Output lines shown before "Show more".
pub(crate) const OUTPUT_CLAMP: usize = 12;
/// Body lines of other tools shown before "Show more".
pub(crate) const TEXT_CLAMP: usize = 6;
/// Diff rows shown before "Show all N lines".
const DIFF_CLAMP: usize = 12;
/// Height of a tool card's header row (the chat's compact header).
const HEAD_H: f32 = 36.;
/// Height of a short tool call's row.
const LINE_H: f32 = 32.;
/// Width of the verb column of a short tool call.
const VERB_W: f32 = 40.;

/// Where a tool call is, as its row says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The model is still writing the call.
    Preparing,
    Running,
    Done,
    Failed,
    /// It never finished: the turn stopped first.
    Interrupted,
}

/// A tool call's phase and, when known, how long it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolStatus {
    pub phase: Phase,
    pub secs: Option<u64>,
}

impl ToolStatus {
    /// The state word with its mark: "running · 0:12", "done · 4s", "failed".
    pub fn element(self) -> AnyElement {
        let (mark, word, color): (AnyElement, &str, gpui::Hsla) = match self.phase {
            Phase::Preparing => (StatusDot::new(Status::Running).small().into_any_element(), "preparing", INFO_TEXT),
            Phase::Running => (StatusDot::new(Status::Running).small().into_any_element(), "running", INFO_TEXT),
            Phase::Done => (icon(IconName::Check, 12., SUCCESS).into_any_element(), "done", SUCCESS),
            Phase::Failed => (icon(IconName::X, 12., DANGER).into_any_element(), "failed", DANGER),
            Phase::Interrupted => (icon(IconName::Pause, 12., TEXT_MUTED).into_any_element(), "interrupted", TEXT_MUTED),
        };
        row()
            .flex_none()
            .gap(px(SPACE_1))
            .type_style(LABEL)
            .text_color(color)
            .child(mark)
            .child(word)
            .children(self.secs.map(|secs| div().text_color(TEXT_MUTED).child(format!("· {}", base::fmt::duration(secs)))))
            .into_any_element()
    }
}

/// The display an interrupted call keeps: the look of its kind (from before the result, when
/// the result replaced it with a generic error card), without the error as its output.
pub(crate) fn interrupted_display(tool: &ToolBlock, before: Option<&ToolDisplay>) -> Option<ToolDisplay> {
    let display = match &tool.display {
        Some(ToolDisplay::Text { title, .. }) if *title == tool.name => {
            before.cloned().or_else(|| Some(ToolDisplay::Text { title: title.clone(), body: String::new() }))
        }
        other => other.clone(),
    }?;
    Some(match display {
        ToolDisplay::Command { command, .. } => ToolDisplay::Command { command, output: String::new(), exit_code: None },
        ToolDisplay::Line { verb, target, .. } => ToolDisplay::Line { verb, target, result: String::new() },
        other => other,
    })
}

/// `edit_file` reads `edit`; a new file reads `write`.
fn diff_verb(name: &str, file: &FileDiff) -> &'static str {
    if file.created || name.contains("write") { "write" } else { "edit" }
}

/// The kit's diff input from the model's.
pub(crate) fn to_diff_file(file: &FileDiff) -> DiffFile {
    DiffFile {
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        added: file.added,
        removed: file.removed,
        created: file.created,
        deleted: file.deleted,
        binary: file.binary,
        hunks: file
            .hunks
            .iter()
            .map(|hunk| DiffHunk {
                lines: hunk
                    .lines
                    .iter()
                    .map(|line| DiffLine {
                        kind: match line.kind {
                            hub::model::DiffLineKind::Context => DiffLineKind::Context,
                            hub::model::DiffLineKind::Add => DiffLineKind::Add,
                            hub::model::DiffLineKind::Del => DiffLineKind::Del,
                        },
                        old_no: line.old_no,
                        new_no: line.new_no,
                        text: line.text.clone(),
                        no_newline: line.no_newline,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// The first `limit` lines of `text` and how many are left out.
pub(crate) fn clamp_lines(text: &str, limit: usize, expanded: bool) -> (String, usize) {
    let total = text.lines().count();
    if expanded || total <= limit {
        return (text.trim_end_matches('\n').to_string(), 0);
    }
    (text.lines().take(limit).collect::<Vec<_>>().join("\n"), total - limit)
}

impl TranscriptView {
    /// Where `tool` is, from its state, the session and what the view timed.
    pub(crate) fn tool_status(&self, tool: &ToolBlock, busy: bool, cx: &App) -> ToolStatus {
        let phase = match tool.state {
            _ if self.model.read(cx).is_interrupted(&tool.id) => Phase::Interrupted,
            ToolState::Started if busy => Phase::Preparing,
            ToolState::Ready if busy => Phase::Running,
            ToolState::Started | ToolState::Ready => Phase::Interrupted,
            ToolState::Finished { is_error: false } => match &tool.display {
                Some(ToolDisplay::Command { exit_code: Some(code), .. }) if *code != 0 => Phase::Failed,
                _ => Phase::Done,
            },
            ToolState::Finished { is_error: true } => Phase::Failed,
        };
        // The block's own times when it has both ends (history included); while it runs, what
        // the view timed.
        let secs = match tool.duration_ms() {
            // Under a second says nothing: history stores a call and its result in one go.
            Some(ms) if ms >= 1000 => Some(ms as u64 / 1000),
            Some(_) => None,
            None if matches!(phase, Phase::Preparing | Phase::Running) => {
                self.timings.get(&tool.id).map(|timing| timing.secs())
            }
            None => None,
        };
        ToolStatus { phase, secs }
    }

    /// One tool call's row: its question while one is pending, else the look of its kind.
    pub(crate) fn tool(
        &mut self,
        index: usize,
        block: &Arc<Block>,
        tool: &ToolBlock,
        place: &Place,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let probe = format!("tool-{}", tool.id);
        if let Some(request) = self.links.pending_on(index).cloned() {
            let selector = probe.clone();
            return div()
                .debug_selector(move || selector)
                .child(self.permission_banner(&request, cx))
                .probe(probe)
                .into_any_element();
        }
        let status = self.tool_status(tool, self.model.read(cx).is_busy(), cx);
        let display = if self.model.read(cx).is_interrupted(&tool.id) {
            interrupted_display(tool, self.call_displays.get(&tool.id))
        } else {
            tool.display.clone()
        };
        let body = match &display {
            None => line_row(place, &tool.name, "", "", status),
            Some(ToolDisplay::Line { verb, target, result }) => line_row(place, verb, target, result, status),
            Some(ToolDisplay::Command { command, output, .. }) => self.command_card(index, block, command, output, status, cx),
            Some(ToolDisplay::Diff(file)) => self.diff_card(block, tool, file, status),
            Some(ToolDisplay::Text { title, body }) => self.text_card(index, block, title, body, status, cx),
        };
        let selector = probe.clone();
        div().debug_selector(move || selector).child(body).probe(probe).into_any_element()
    }

    /// A command: `$ command`, its state, copy and fold buttons, then its output clamped to
    /// [`OUTPUT_CLAMP`] lines behind "Show more".
    fn command_card(
        &mut self,
        index: usize,
        block: &Block,
        command: &str,
        output: &str,
        status: ToolStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = block.id;
        let ui = self.row_ui(id);
        let has_output = !output.trim().is_empty();
        let open = has_output && ui.open.unwrap_or(true);
        let copy_text = if has_output { format!("$ {command}\n{output}") } else { command.to_string() };
        let copy = IconButton::new(ElementId::NamedInteger("copy-output".into(), id), IconName::Copy)
            .small()
            .tooltip(if has_output { "Copy command and output" } else { "Copy command" })
            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone())));
        let toggle = has_output.then(|| {
            IconButton::new(
                ElementId::NamedInteger("toggle-output".into(), id),
                if open { IconName::ChevronDown } else { IconName::ChevronRight },
            )
            .small()
            .tooltip(if open { "Hide output" } else { "Show output" })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(id, true, cx)))
            .probe(format!("toggle-{index}"))
        });
        let header = row()
            .flex_none()
            .h(px(HEAD_H))
            .pl(px(SPACE_3))
            .pr(px(SPACE_1))
            .gap(px(SPACE_2))
            .child(icon(IconName::Terminal, 12., TEXT_MUTED))
            .child(truncate(format!("$ {command}")).type_style(MONO).text_color(TEXT))
            .child(status.element())
            .child(copy)
            .children(toggle);
        let (shown, hidden) = clamp_lines(output, OUTPUT_CLAMP, ui.expanded);
        let body = open.then(|| {
            div()
                .flex_none()
                .px(px(SPACE_3))
                .py(px(SPACE_1_5))
                .bg(BG_SIDEBAR)
                .border_t_1()
                .border_color(LINE)
                .type_style(MONO)
                .text_color(TEXT_SECONDARY)
                .overflow_hidden()
                .child(shown)
        });
        let more = (open && hidden > 0).then(|| self.show_more(index, id, hidden, cx));
        col()
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .overflow_hidden()
            .child(header)
            .children(body)
            .children(more)
            .into_any_element()
    }

    /// The "Show more" strip under clamped text.
    fn show_more(&self, index: usize, id: u64, hidden: usize, cx: &mut Context<Self>) -> AnyElement {
        row()
            .flex_none()
            .gap(px(SPACE_2))
            .h(px(CONTROL_HEIGHT_MD))
            .px(px(SPACE_3))
            .bg(BG_SIDEBAR)
            .border_t_1()
            .border_color(LINE_SUBTLE)
            .type_style(LABEL)
            .child(
                Link::new(ElementId::Name(format!("show-more-{index}").into()), "Show more")
                    .on_click(cx.listener(move |this, _, _, cx| this.expand(id, cx)))
                    .probe(format!("show-more-{index}")),
            )
            .child(div().text_color(TEXT_MUTED).child(base::fmt::plural(hidden as u64, "more line", "more lines")))
            .into_any_element()
    }

    /// An edit or a new file: the kit's diff card with `edit`, the path and `+3 −2`.
    fn diff_card(&mut self, block: &Arc<Block>, tool: &ToolBlock, file: &FileDiff, status: ToolStatus) -> AnyElement {
        let diff = self.diff_file(block, file);
        DiffCard::new(ElementId::NamedInteger("edit".into(), block.id), diff)
            .label(diff_verb(&tool.name, file))
            .compact()
            .collapse_after(DIFF_CLAMP)
            .trailing(status.element())
            .into_any_element()
    }

    /// The kit's diff of a block, converted once per change of the block.
    fn diff_file(&mut self, block: &Arc<Block>, file: &FileDiff) -> Arc<DiffFile> {
        if let Some((cached, diff)) = self.diffs.get(&block.id)
            && Arc::ptr_eq(cached, block)
        {
            return diff.clone();
        }
        let diff = Arc::new(to_diff_file(file));
        self.diffs.insert(block.id, (block.clone(), diff.clone()));
        diff
    }

    /// Any other tool: its title and state, then its text clamped to [`TEXT_CLAMP`] lines.
    fn text_card(
        &mut self,
        index: usize,
        block: &Block,
        title: &str,
        body: &str,
        status: ToolStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = self.row_ui(block.id);
        let header = row()
            .flex_none()
            .h(px(HEAD_H))
            .px(px(SPACE_3))
            .gap(px(SPACE_2))
            .child(icon(IconName::Zap, 12., TEXT_MUTED))
            .child(truncate(SharedString::from(title.to_string())).type_style(MONO).text_color(TEXT_SECONDARY))
            .child(status.element());
        let (shown, hidden) = clamp_lines(body, TEXT_CLAMP, ui.expanded);
        let text = (!shown.trim().is_empty()).then(|| {
            div()
                .flex_none()
                .px(px(SPACE_3))
                .py(px(SPACE_2))
                .border_t_1()
                .border_color(LINE_SUBTLE)
                .type_style(LABEL)
                .text_color(if status.phase == Phase::Failed { DANGER } else { TEXT_SECONDARY })
                .child(shown)
        });
        let more = (hidden > 0).then(|| self.show_more(index, block.id, hidden, cx));
        col()
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .overflow_hidden()
            .child(header)
            .children(text)
            .children(more)
            .into_any_element()
    }
}

/// A short call as one row of the shared "N tool calls" card: verb, target, result.
fn line_row(place: &Place, verb: &str, target: &str, result: &str, status: ToolStatus) -> AnyElement {
    let outcome: AnyElement = match status.phase {
        Phase::Done if !result.is_empty() => {
            div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(result.to_string()).into_any_element()
        }
        Phase::Failed if !result.is_empty() => {
            div().flex_none().type_style(LABEL).text_color(DANGER).child(result.to_string()).into_any_element()
        }
        _ => status.element(),
    };
    let line = row()
        .flex_none()
        .h(px(LINE_H))
        .px(px(SPACE_3))
        .gap(px(SPACE_2))
        .border_t_1()
        .border_color(LINE_SUBTLE)
        .child(div().flex_none().min_w(px(VERB_W)).type_style(MONO).text_color(TEXT_MUTED).child(verb.to_string()))
        .child(truncate(target.to_string()).type_style(MONO).text_color(TEXT))
        .child(outcome);
    let group = place.group.unwrap_or(super::rows::GroupPlace { first: true, last: true, size: 1 });
    let header = row()
        .flex_none()
        .h(px(LIST_ITEM_HEIGHT))
        .px(px(SPACE_3))
        .gap(px(SPACE_2))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(icon(IconName::Terminal, 12., TEXT_MUTED))
        .child(base::fmt::plural(group.size as u64, "tool call", "tool calls"))
        .into_any_element();
    group_card(group, LINE, Some(header), line.into_any_element())
}
