//! The selectable texts of rows that are not messages: a command and its output, a tool's
//! text, an error, a sub-agent's summary, what a permission question wants to run. Each is a
//! `Markdown` view (verbatim, or Markdown for model prose) kept per row part, so a selection
//! survives frames and streaming, and dropped once its block or question is gone.

use std::collections::HashSet;

use gpui::{AppContext as _, Context, Entity, Hsla, SharedString};
use hub::model::RequestId;
use hub::transcript::{BlockId, BlockKind, PermissionState};
use ui::prelude::*;
use ui_rich::markdown::Markdown;

use super::TranscriptView;

/// Which part of a row a text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    /// A command line.
    Command,
    /// A command's output.
    Output,
    /// The text of any other tool.
    Body,
    /// An error: a failed short call's result, a failed turn, a notice.
    Error,
    /// A finished sub-agent's summary.
    Summary,
}

/// One selectable text of the transcript.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TextKey {
    /// A part of block `id`'s row.
    Block(BlockId, Part),
    /// What a pending permission question wants to run.
    Request(RequestId),
    /// Why the chat could not be loaded.
    LoadError,
}

/// How a text reads: exactly as given, or as Markdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Verbatim,
    Markdown,
}

impl TranscriptView {
    /// The selectable text `key` showing `text` in `style` and `color`, made on first use and
    /// brought up to date after (a growing text only adds its new part).
    pub(crate) fn text_view(
        &mut self,
        key: TextKey,
        text: &str,
        mode: Mode,
        style: TypeStyle,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> Entity<Markdown> {
        if let Some(md) = self.texts.get(&key) {
            let md = md.clone();
            md.update(cx, |md, cx| {
                if md.text() != text {
                    md.set_text(SharedString::from(text.to_string()), cx);
                }
                md.set_type_style(style, cx);
                md.set_color(color, cx);
            });
            return md;
        }
        let source = SharedString::from(text.to_string());
        let base = self.image_base(cx);
        let md = cx.new(|cx| {
            let md = match mode {
                Mode::Verbatim => Markdown::verbatim(source, cx),
                Mode::Markdown => {
                    let md = Markdown::new(source, cx).on_link(|url, _, cx| cx.open_url(url));
                    match base {
                        Some(dir) => md.image_base(dir),
                        None => md,
                    }
                }
            };
            md.type_style(style).color(color)
        });
        self.texts.insert(key, md.clone());
        md
    }

    /// [`text_view`](Self::text_view) for text shown exactly as given.
    pub(crate) fn verbatim(
        &mut self,
        key: TextKey,
        text: &str,
        style: TypeStyle,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> Entity<Markdown> {
        self.text_view(key, text, Mode::Verbatim, style, color, cx)
    }

    /// Drops the texts of blocks that are gone and of questions no longer pending.
    pub(crate) fn prune_texts(&mut self, cx: &Context<Self>) {
        let model = self.model.read(cx);
        let blocks: HashSet<BlockId> = model.blocks().iter().map(|block| block.id).collect();
        let asked = model.blocks().iter().filter_map(|block| match &block.kind {
            BlockKind::Permission(question) if matches!(question.state, PermissionState::Pending) => {
                Some(&question.request.id)
            }
            _ => None,
        });
        let pending: HashSet<&RequestId> = model.pending().iter().map(|request| &request.id).chain(asked).collect();
        let loaded = !matches!(model.state(), hub::LoadState::Failed(_));
        self.texts.retain(|key, _| match key {
            TextKey::Block(id, _) => blocks.contains(id),
            TextKey::Request(id) => pending.contains(id),
            TextKey::LoadError => !loaded,
        });
        self.markdown.retain(|id, _| blocks.contains(id));
    }
}
