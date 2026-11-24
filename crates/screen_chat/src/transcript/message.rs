//! Messages: what the person sent (with images and `@` mentions), the agent's text as
//! Markdown while it streams, its thinking (folded once finished), and the header that opens
//! each article.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Hsla, IntoElement, ObjectFit, SharedString,
    StyledImage, Window, div, img, prelude::*, px,
};
use hub::Block;
use hub::model::Attachment;
use hub::transcript::{BlockKind, TextBlock, UserBlock};
use ui::dev::Probe;
use ui::motion::Look;
use ui::prelude::*;
use ui_rich::markdown::Markdown;

use super::TranscriptView;
use super::rows::INDENT;

/// Size of an image thumbnail in a message (`.thumb`).
const THUMB_W: f32 = 132.;
const THUMB_H: f32 = 84.;
/// Height of a thumbnail's caption strip.
const CAPTION_H: f32 = 22.;

/// The Markdown source a block shows, for the blocks drawn as Markdown.
pub(crate) fn markdown_text(kind: &BlockKind) -> Option<&str> {
    match kind {
        BlockKind::User(user) => Some(&user.text),
        BlockKind::Assistant(text) | BlockKind::Thinking(text) => Some(&text.text),
        _ => None,
    }
}

/// The `@name` words of a message, without trailing punctuation: what Markdown draws as
/// mentions.
pub(crate) fn mentions(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(name) = word.strip_prefix('@') else { continue };
        let name = name.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']);
        if !name.is_empty() && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// Initial capital: `build` reads `Build`.
pub(crate) fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl TranscriptView {
    /// The Markdown view of block `id`, made on first use.
    fn markdown_for(
        &mut self,
        id: u64,
        text: &str,
        color: Hsla,
        user: bool,
        cx: &mut Context<Self>,
    ) -> Entity<Markdown> {
        if let Some(md) = self.markdown.get(&id) {
            return md.clone();
        }
        let source = SharedString::from(text.to_string());
        let names = if user { mentions(text) } else { Vec::new() };
        let md = cx.new(|cx| {
            Markdown::new(source, cx).color(color).mentions(names).on_link(|url, _, cx| cx.open_url(url))
        });
        self.markdown.insert(id, md.clone());
        md
    }

    /// Who answers in this session, as the header names it: "Build".
    pub(crate) fn agent_name(&self, cx: &App) -> String {
        let Some(session) = self.model.read(cx).session() else { return "Agent".to_string() };
        self.catalog
            .read(cx)
            .runnable(&session.runnable)
            .map(|runnable| runnable.name.clone())
            .unwrap_or_else(|| capitalised(session.runnable.id()))
    }

    /// "2h": how long ago `at` was.
    pub(crate) fn ago(&self, at: i64) -> String {
        let now = self.now();
        base::time::ago(now, at, base::time::utc_offset(now))
    }

    /// The person's message: avatar, "You", time, then the card with images and text.
    pub(crate) fn user_message(
        &mut self,
        block: &Block,
        user: &UserBlock,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = row()
            .gap(px(SPACE_2))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(Avatar::new("ME").large())
            .child(div().type_style(LABEL_STRONG).text_color(TEXT).child("You"))
            .child(self.ago(user.at));
        let queries = hub::queries(cx);
        let thumbs: Vec<AnyElement> =
            user.images.iter().enumerate().map(|(i, image)| thumb(block.id, i, image, &queries)).collect();
        let text = (!user.text.is_empty()).then(|| self.markdown_for(block.id, &user.text, TEXT, true, cx));
        let card = col()
            .ml(px(INDENT))
            .gap(px(SPACE_2_5))
            .py(px(SPACE_3))
            .px(px(SPACE_3_5))
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .when(!thumbs.is_empty(), |d| d.child(row().gap(px(SPACE_2)).overflow_hidden().children(thumbs)))
            .children(text);
        col().gap(px(SPACE_3)).child(header).child(card).into_any_element()
    }

    /// The header of an agent's turn: tile, name, model, time.
    pub(crate) fn agent_header(&self, blocks: &[Arc<Block>], index: usize, cx: &App) -> AnyElement {
        let turn = blocks[index..].iter().take_while(|block| !matches!(block.kind, BlockKind::User(_)));
        let mut model = None;
        let mut at = None;
        for block in turn {
            if let BlockKind::Assistant(text) | BlockKind::Thinking(text) = &block.kind {
                model = model.or_else(|| text.model.clone());
                at = at.or(text.at);
                if model.is_some() && at.is_some() {
                    break;
                }
            }
        }
        let session = self.model.read(cx).session();
        let model = model.or_else(|| session.map(|session| session.model.clone()));
        row()
            .gap(px(SPACE_2))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .child(Tile::new(IconName::Agent))
            .child(div().type_style(LABEL_STRONG).text_color(TEXT).child(self.agent_name(cx)))
            .children(model.filter(|m| !m.is_empty()).map(|m| div().type_style(MONO).child(m.to_string())))
            .children(at.map(|at| div().child(self.ago(at))))
            .into_any_element()
    }

    /// The agent's text, with a caret while it streams.
    pub(crate) fn assistant_text(&mut self, block: &Block, text: &TextBlock, cx: &mut Context<Self>) -> AnyElement {
        let md = self.markdown_for(block.id, &text.text, TEXT_SECONDARY, false, cx);
        col()
            .child(md)
            .when(text.streaming, |d| {
                d.child(div().mt(px(SPACE_1)).w(px(SPACE_0_5)).h(px(SPACE_3_5)).bg(ACCENT).probe("streaming-caret"))
            })
            .into_any_element()
    }

    /// The agent's reasoning: open while it streams, folded once finished, toggled by its
    /// header.
    pub(crate) fn thinking(
        &mut self,
        index: usize,
        block: &Block,
        text: &TextBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let default_open = text.streaming;
        let open = self.row_ui(block.id).open.unwrap_or(default_open);
        let id = block.id;
        let key = ElementId::NamedInteger("thinking-toggle".into(), id);
        let fade = motion::hover_fade_state(
            key.clone(),
            Look::bg(TRANSPARENT).fg(TEXT_MUTED),
            Look::bg(TRANSPARENT).fg(TEXT_SECONDARY),
            window,
            cx,
        );
        let fg = fade.fg.unwrap_or(TEXT_MUTED);
        let toggle = fade
            .apply(div().id(key).flex().flex_row().items_center().gap(px(SPACE_1_5)).h(px(LABEL.line)).cursor_pointer())
            .debug_selector(move || format!("thinking-{index}"))
            .type_style(LABEL)
            .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., fg))
            .child(if text.streaming { "Thinking…" } else { "Thinking" })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(id, default_open, cx)))
            .probe(format!("thinking-{index}"));
        let body = open.then(|| {
            let md = self.markdown_for(block.id, &text.text, TEXT_MUTED, false, cx);
            div().pl(px(SPACE_3)).border_l_1().border_color(LINE).child(md)
        });
        col().gap(px(SPACE_2)).child(row().child(toggle)).children(body).into_any_element()
    }
}

/// One image of a message: the picture over a caption with its name and size.
fn thumb(block: u64, index: usize, image: &Attachment, queries: &hub::Queries) -> AnyElement {
    let path = queries.blob_path(&image.blob);
    let shot = div()
        .absolute()
        .top(px(SPACE_2))
        .left(px(SPACE_2))
        .right(px(SPACE_2))
        .bottom(px(CAPTION_H + SPACE_1))
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(LINE)
        .bg(SURFACE)
        .overflow_hidden()
        .child(img(path).size_full().object_fit(ObjectFit::Cover));
    let caption = row()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(CAPTION_H))
        .gap(px(SPACE_1_5))
        .px(px(SPACE_2))
        .type_style(CAPTION)
        .text_color(TEXT_SECONDARY)
        .bg(SURFACE_RAISED)
        .border_t_1()
        .border_color(LINE)
        .child(icon(IconName::Image, 12., TEXT_SECONDARY))
        .child(truncate_lh(image.name.clone(), CAPTION.line))
        .child(div().flex_none().text_color(TEXT_MUTED).child(base::fmt::bytes(image.bytes)));
    div()
        .id(ElementId::NamedInteger(format!("thumb-{block}").into(), index as u64))
        .relative()
        .flex_none()
        .w(px(THUMB_W))
        .h(px(THUMB_H))
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(RADIUS_LG))
        .bg(BG_SIDEBAR)
        .overflow_hidden()
        .child(shot)
        .child(caption)
        .probe_value(format!("image-{block}-{index}"), image.name.clone())
        .into_any_element()
}
