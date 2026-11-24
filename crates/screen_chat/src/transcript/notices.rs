//! Lines that are not the conversation: the compaction divider, notices (retry, interrupted,
//! pull request), a failed turn with Retry, the footer with queued messages and the working
//! line, and the whole-view states (loading, failed to load, empty, deleted).

use std::sync::Arc;

use gpui::{AnyElement, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::Block;
use hub::transcript::{BlockKind, CompactionBlock, NoticeBlock, NoticeKind};
use ui::dev::Probe;
use ui::prelude::*;

use super::TranscriptView;
use super::rows::{COLUMN_MAX, INDENT};

/// "30 older messages summarised", or the running line.
pub(crate) fn compaction(block: CompactionBlock) -> AnyElement {
    let label: AnyElement = if block.running {
        row()
            .gap(px(SPACE_1_5))
            .child(StatusDot::new(Status::Running).small())
            .child("Summarising older messages")
            .into_any_element()
    } else {
        div()
            .child(format!("{} summarised", base::fmt::plural(u64::from(block.summarised), "older message", "older messages")))
            .into_any_element()
    };
    row()
        .gap(px(SPACE_3))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(hr().flex_1())
        .child(div().flex_none().child(label))
        .child(hr().flex_1())
        .probe("compaction")
        .into_any_element()
}

impl TranscriptView {
    /// A notice line: its mark and text; a pull request's URL is a link.
    pub(crate) fn notice(&mut self, block: &Block, notice: &NoticeBlock, _cx: &mut Context<Self>) -> AnyElement {
        let (mark, color) = match notice.kind {
            NoticeKind::Info => (IconName::Alert, TEXT_MUTED),
            NoticeKind::Retry => (IconName::Replay, TEXT_MUTED),
            NoticeKind::Interrupted => (IconName::Pause, TEXT_MUTED),
            NoticeKind::PullRequest => (IconName::Branch, TEXT_MUTED),
        };
        let text: AnyElement = match notice.kind {
            NoticeKind::PullRequest => {
                let url = notice.text.clone();
                row()
                    .gap(px(SPACE_1_5))
                    .child("Pull request opened")
                    .child(
                        Link::new(ElementId::NamedInteger("pr-link".into(), block.id), url.clone())
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .into_any_element()
            }
            _ => truncate(notice.text.clone()).into_any_element(),
        };
        row()
            .gap(px(SPACE_2))
            .type_style(LABEL)
            .text_color(color)
            .child(icon(mark, 12., color))
            .child(text)
            .into_any_element()
    }

    /// A failed turn: the cause, and Retry when it is the last thing that happened.
    pub(crate) fn failure(
        &mut self,
        blocks: &[Arc<Block>],
        index: usize,
        error: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let latest = blocks[index + 1..].iter().all(|block| matches!(block.kind, BlockKind::Notice(_)));
        let busy = self.model.read(cx).is_busy();
        let retry = (latest && !busy).then(|| {
            Button::new("retry-turn", "Retry")
                .small()
                .icon(IconName::Replay)
                .on_click(cx.listener(|this, _, _, cx| hub::commands(cx).retry(&this.session, cx)))
                .probe("retry-turn")
        });
        Banner::new()
            .tone(BannerTone::Danger)
            .child(icon(IconName::Alert, 16., DANGER))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(div().type_style(BODY_STRONG).text_color(TEXT).child("The turn failed"))
                    .child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child(error.to_string())),
            )
            .children(retry)
            .into_any_element()
    }

    /// The last row: messages queued for the next turn and the working line.
    pub(crate) fn footer(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.read(cx);
        let busy = model.is_busy();
        let waiting = !model.pending().is_empty();
        let streaming = model.blocks().last().is_some_and(|block| {
            matches!(&block.kind, BlockKind::Assistant(text) | BlockKind::Thinking(text) if text.streaming)
        });
        let queued: Vec<AnyElement> = model
            .queued()
            .iter()
            .enumerate()
            .map(|(i, message)| {
                let images = message.images.len();
                row()
                    .id(ElementId::NamedInteger("queued".into(), i as u64))
                    .ml(px(INDENT))
                    .gap(px(SPACE_2))
                    .h(px(CONTROL_HEIGHT_MD))
                    .px(px(SPACE_3))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .rounded(px(RADIUS_LG))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(icon(IconName::Clock, 12., TEXT_MUTED))
                    .child(div().flex_none().text_color(TEXT_SECONDARY).child("Queued"))
                    .child(truncate(SharedString::from(message.text.clone())))
                    .when(images > 0, |d| d.child(div().flex_none().child(base::fmt::plural(images as u64, "image", "images"))))
                    .probe(format!("queued-{i}"))
                    .into_any_element()
            })
            .collect();
        let working = (busy && !waiting && !streaming).then(|| {
            row()
                .ml(px(INDENT))
                .gap(px(SPACE_2))
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(StatusDot::new(Status::Running).small())
                .child(format!("{} is working", self.agent_name(cx)))
                .probe("working")
        });
        let has_content = !queued.is_empty() || working.is_some();
        div()
            .id("transcript-footer")
            .w_full()
            .flex()
            .justify_center()
            .child(
                col()
                    .w_full()
                    .max_w(px(COLUMN_MAX))
                    .px(px(SPACE_6))
                    .when(has_content, |d| d.pt(px(SPACE_3)))
                    .pb(px(SPACE_6))
                    .gap(px(SPACE_2))
                    .children(working)
                    .children(queued),
            )
            .into_any_element()
    }
}

/// The whole view while the first page loads.
pub(crate) fn loading_state() -> AnyElement {
    centred(
        "transcript-loading",
        row()
            .gap(px(SPACE_2))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(StatusDot::new(Status::Running).small())
            .child("Loading messages")
            .probe("transcript-loading")
            .into_any_element(),
    )
}

/// The whole view when the chat could not be read.
pub(crate) fn error_state(error: SharedString) -> AnyElement {
    centred(
        "transcript-error",
        Banner::new()
            .tone(BannerTone::Danger)
            .child(icon(IconName::Alert, 16., DANGER))
            .child(
                col()
                    .min_w_0()
                    .child(div().type_style(BODY_STRONG).text_color(TEXT).child("Could not load this chat"))
                    .child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child(error)),
            )
            .probe("transcript-error")
            .into_any_element(),
    )
}

/// The whole view of a chat with no message yet.
pub(crate) fn empty_state() -> AnyElement {
    centred(
        "transcript-empty",
        col()
            .items_center()
            .gap(px(SPACE_2))
            .child(Tile::new(IconName::Chat).large())
            .child(div().type_style(HEADING).text_color(TEXT).child("No messages yet"))
            .child(Note::new("Send a message to start the chat."))
            .probe("transcript-empty")
            .into_any_element(),
    )
}

/// The whole view of a chat deleted while open.
pub(crate) fn removed_state() -> AnyElement {
    centred(
        "transcript-removed",
        col()
            .items_center()
            .gap(px(SPACE_2))
            .child(Tile::new(IconName::Trash).large())
            .child(div().type_style(HEADING).text_color(TEXT).child("This chat was deleted"))
            .probe("transcript-removed")
            .into_any_element(),
    )
}

/// `content` in the middle of the view, findable in tests as `name`.
fn centred(name: &'static str, content: AnyElement) -> AnyElement {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(px(SPACE_6))
        .debug_selector(move || name.to_string())
        .child(content)
        .into_any_element()
}
