//! The frame of every row: the centred column, the spacing of articles (a person's message, an
//! agent's turn) and of rows inside them, and where a row sits in a group of tool calls or
//! sub-agents that share one card.

use std::sync::Arc;

use gpui::{AnyElement, Context, ElementId, IntoElement, Window, div, prelude::*, px};
use hub::Block;
use hub::model::ToolDisplay;
use hub::transcript::BlockKind;
use ui::dev::Probe;
use ui::prelude::*;

use super::TranscriptView;
use super::links::Links;

/// Widest the transcript column gets, padding included (the mockup's 760 px of text).
pub(crate) const COLUMN_MAX: f32 = 808.;
/// Space between articles: a message and the turn that answers it.
pub(crate) const ARTICLE_GAP: f32 = 28.;
/// How far an article's body sits right of its header (avatar plus gap).
pub(crate) const INDENT: f32 = SPACE_8;

/// Rows that share one card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    /// Short tool calls (`read`, `grep`): "3 tool calls".
    Tools,
    /// Sub-agents: "Build started 2 sub-agents".
    Agents,
}

/// Where a row sits in its group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GroupPlace {
    pub first: bool,
    pub last: bool,
    /// Rows of the group; counted on the first row only (0 elsewhere).
    pub size: usize,
}

/// How a row is framed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Place {
    /// Space above the row.
    pub top: f32,
    /// The row starts an agent's turn: draw the header above it.
    pub header: bool,
    pub group: Option<GroupPlace>,
}

/// The group row `index` belongs to, if any.
pub(crate) fn group_of(blocks: &[Arc<Block>], links: &Links, index: usize) -> Option<Group> {
    match &blocks.get(index)?.kind {
        BlockKind::Tool(tool) if links.pending_on(index).is_none() => match &tool.display {
            None | Some(ToolDisplay::Line { .. }) => Some(Group::Tools),
            Some(_) => None,
        },
        BlockKind::Subagent(_) => Some(Group::Agents),
        _ => None,
    }
}

/// The first row of the group that row `index - 1` is in, or `index` when it is in none:
/// the rows before a change whose look depends on what follows them.
pub(crate) fn group_start(blocks: &[Arc<Block>], links: &Links, index: usize) -> usize {
    let Some(prev) = index.checked_sub(1) else { return index };
    let Some(group) = group_of(blocks, links, prev) else { return index };
    let mut start = prev;
    while start > 0 && group_of(blocks, links, start - 1) == Some(group) {
        start -= 1;
    }
    start
}

/// One past the last row of the group that starts at `index`, or `index + 1`: the rows after
/// a change whose look depends on what precedes them.
pub(crate) fn group_end(blocks: &[Arc<Block>], links: &Links, index: usize) -> usize {
    if index >= blocks.len() {
        return index;
    }
    let Some(group) = group_of(blocks, links, index) else { return index + 1 };
    let mut end = index + 1;
    while group_of(blocks, links, end) == Some(group) {
        end += 1;
    }
    end
}

/// The nearest row before `index` that draws something.
pub(crate) fn prev_visible(links: &Links, index: usize) -> Option<usize> {
    (0..index).rev().find(|&i| !links.is_hidden(i))
}

/// How row `index` is framed.
pub(crate) fn place(blocks: &[Arc<Block>], links: &Links, index: usize) -> Place {
    let kind = &blocks[index].kind;
    let group = group_of(blocks, links, index).map(|group| {
        let first = index == 0 || group_of(blocks, links, index - 1) != Some(group);
        let last = group_of(blocks, links, index + 1) != Some(group);
        let size = if first { group_end(blocks, links, index) - index } else { 0 };
        GroupPlace { first, last, size }
    });
    let prev = prev_visible(links, index);
    let user = matches!(kind, BlockKind::User(_));
    let divider = matches!(kind, BlockKind::Compaction(_));
    let header = !user
        && !divider
        && prev.is_none_or(|p| matches!(blocks[p].kind, BlockKind::User(_) | BlockKind::Compaction(_)));
    let top = match prev {
        None => SPACE_6,
        Some(_) if group.is_some_and(|g| !g.first) => 0.,
        Some(_) if user || header || divider => ARTICLE_GAP,
        Some(_) => SPACE_3,
    };
    Place { top, header, group }
}

/// The column's width in a list `available` px wide: [`COLUMN_MAX`] or the whole list.
pub(crate) fn column_width(available: f32) -> f32 {
    available.clamp(0., COLUMN_MAX)
}

/// The centred column around one row's content, keyed by the block's id so a row keeps its
/// element state while it streams.
///
/// The column gets a fixed width, not `w_full().max_w(..)`: list rows are laid out as roots
/// at the list's width, and with a relative width taffy measures wrapped text (Markdown) at
/// the wrong width, giving rows thousands of pixels tall or text overflowing its row.
pub(crate) fn frame(width: f32, index: usize, block: &Block, top: f32, content: impl IntoElement) -> AnyElement {
    div()
        .id(ElementId::NamedInteger("block".into(), block.id))
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .debug_selector(|| format!("block-{index}"))
        .child(col().flex_none().w(px(width)).px(px(SPACE_6)).pt(px(top)).child(content))
        .probe(format!("block-{index}"))
        .into_any_element()
}

/// Row `index`, framed.
pub(crate) fn row(
    view: &mut TranscriptView,
    blocks: &[Arc<Block>],
    index: usize,
    window: &mut Window,
    cx: &mut Context<TranscriptView>,
) -> AnyElement {
    let block = blocks[index].clone();
    let width = view.column_width;
    if view.links.is_hidden(index) {
        return frame(width, index, &block, 0., div());
    }
    let place = place(blocks, &view.links, index);
    let content: AnyElement = match &block.kind {
        BlockKind::User(user) => return frame(width, index, &block, place.top, view.user_message(index, &block, user, window, cx)),
        BlockKind::Compaction(compaction) => {
            return frame(width, index, &block, place.top, super::notices::compaction(*compaction));
        }
        BlockKind::Assistant(text) => view.assistant_text(&block, text, cx),
        BlockKind::Thinking(text) => view.thinking(index, &block, text, window, cx),
        BlockKind::Tool(tool) => view.tool(index, &block, tool, &place, cx),
        BlockKind::Subagent(sub) => view.subagent(index, &block, sub, &place, cx),
        BlockKind::Todos(items) => super::agents::todos(items),
        BlockKind::Permission(question) => view.standalone_permission(index, question, cx),
        BlockKind::Notice(notice) => view.notice(index, &block, notice, cx),
        BlockKind::Failure(error) => view.failure(blocks, index, error, cx),
    };
    let body = div().ml(px(INDENT)).flex().flex_col().child(content);
    if place.header {
        let header = view.agent_header(blocks, index, cx);
        frame(width, index, &block, place.top, col().gap(px(SPACE_3)).child(header).child(body))
    } else {
        frame(width, index, &block, place.top, body)
    }
}

/// A group member's slice of the shared card: side borders, the top with its header on the
/// first row, the bottom on the last, a hairline between rows.
pub(crate) fn group_card(group: GroupPlace, line: gpui::Hsla, header: Option<AnyElement>, content: AnyElement) -> AnyElement {
    col()
        .bg(SURFACE)
        .border_l_1()
        .border_r_1()
        .border_color(line)
        .overflow_hidden()
        .when(group.first, |d| d.border_t_1().rounded_t(px(RADIUS_XL)))
        .when(group.last, |d| d.border_b_1().rounded_b(px(RADIUS_XL)))
        .children(header.filter(|_| group.first))
        .child(content)
        .into_any_element()
}
