//! Markdown for assistant messages: pulldown-cmark events → a small block tree → gpui
//! elements (paragraphs, headings, lists, quotes, code blocks, tables, inline styles).

use gpui::{
    AnyElement, FontStyle, FontWeight, Hsla, IntoElement, StrikethroughStyle, StyledText, TextRun,
    div, font, prelude::*, px,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ui::theme::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading(u8, Vec<Span>),
    Code {
        lang: String,
        text: String,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Table {
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Rule,
}

enum Frame {
    Blocks(Vec<Block>),
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Item(Vec<Block>),
}

#[derive(Default)]
struct TableState {
    head: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
    row: Vec<Vec<Span>>,
}

struct Builder {
    stack: Vec<Frame>,
    inline: Vec<Span>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: u32,
    code: Option<(String, String)>,
    heading: Option<u8>,
    table: Option<TableState>,
}

impl Builder {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: false,
            link: self.link > 0,
        }
    }

    fn push_span(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.inline.last_mut()
            && last.style == style
        {
            last.text.push_str(text);
            return;
        }
        self.inline.push(Span {
            text: text.to_string(),
            style,
        });
    }

    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Blocks(b) | Frame::Quote(b) | Frame::Item(b)) => b.push(block),
            // Blocks directly inside a list (shouldn't happen): wrap in an item.
            Some(Frame::List { items, .. }) => items.push(vec![block]),
            None => self.stack.push(Frame::Blocks(vec![block])),
        }
    }

    /// Loose inline text (tight list items have no Paragraph wrapper) becomes a paragraph.
    fn flush_inline(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let mut spans = std::mem::take(&mut self.inline);
        if let Some(last) = spans.last_mut() {
            let trimmed = last.text.trim_end().len();
            last.text.truncate(trimmed);
        }
        spans.retain(|s| !s.text.is_empty());
        if !spans.is_empty() {
            self.push_block(Block::Paragraph(spans));
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.flush_inline(),
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.heading = Some(heading_level(level));
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.stack.push(Frame::Quote(Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").into(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush_inline();
                self.stack.push(Frame::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.flush_inline();
                self.stack.push(Frame::Item(Vec::new()));
            }
            Tag::Table(_) => {
                self.flush_inline();
                self.table = Some(TableState::default());
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } | Tag::Image { .. } => self.link += 1,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_inline(),
            TagEnd::Heading(_) => {
                let spans = std::mem::take(&mut self.inline);
                let level = self.heading.take().unwrap_or(3);
                if !spans.is_empty() {
                    self.push_block(Block::Heading(level, spans));
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                if let Some(Frame::Quote(blocks)) = self.stack.pop() {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, mut text)) = self.code.take() {
                    while text.ends_with('\n') {
                        text.pop();
                    }
                    self.push_block(Block::Code { lang, text });
                }
            }
            TagEnd::List(_) => {
                self.flush_inline();
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                if let Some(Frame::Item(blocks)) = self.stack.pop() {
                    match self.stack.last_mut() {
                        Some(Frame::List { items, .. }) => items.push(blocks),
                        _ => blocks.into_iter().for_each(|b| self.push_block(b)),
                    }
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(t) = self.table.as_mut() {
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.head = std::mem::take(&mut t.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.push_block(Block::Table {
                        head: t.head,
                        rows: t.rows,
                    });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => self.link = self.link.saturating_sub(1),
            _ => {}
        }
    }
}

fn heading_level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Parse markdown (possibly incomplete, while streaming) into blocks.
pub fn parse(src: &str) -> Vec<Block> {
    let mut b = Builder {
        stack: vec![Frame::Blocks(Vec::new())],
        inline: Vec::new(),
        bold: 0,
        italic: 0,
        strike: 0,
        link: 0,
        code: None,
        heading: None,
        table: None,
    };
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => b.start(tag),
            Event::End(tag) => b.end(tag),
            Event::Text(t) => {
                if let Some((_, code)) = b.code.as_mut() {
                    code.push_str(&t);
                } else {
                    let style = b.style();
                    b.push_span(&t, style);
                }
            }
            Event::Code(t) => {
                let style = Style {
                    code: true,
                    ..b.style()
                };
                b.push_span(&t, style);
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                if let Some((_, code)) = b.code.as_mut() {
                    code.push_str(&t);
                } else {
                    let style = b.style();
                    b.push_span(&t, style);
                }
            }
            // Chat convention (like GitHub comments / Claude): keep single newlines as line
            // breaks instead of folding them into spaces — poems, addresses, step lists.
            Event::SoftBreak | Event::HardBreak => {
                let style = b.style();
                b.push_span("\n", style);
            }
            Event::Rule => {
                b.flush_inline();
                b.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                let style = b.style();
                b.push_span(if done { "☑ " } else { "☐ " }, style);
            }
            _ => {}
        }
    }
    b.flush_inline();
    // Unclosed containers (streaming): fold them back into their parents.
    while b.stack.len() > 1 {
        let block = match b.stack.pop() {
            Some(Frame::Quote(blocks)) => Some(Block::Quote(blocks)),
            Some(Frame::List { start, items }) => Some(Block::List { start, items }),
            Some(Frame::Item(blocks)) => {
                if let Some(Frame::List { items, .. }) = b.stack.last_mut() {
                    items.push(blocks);
                    None
                } else {
                    Some(Block::Quote(blocks))
                }
            }
            Some(Frame::Blocks(blocks)) => Some(Block::Quote(blocks)),
            None => None,
        };
        if let Some(block) = block {
            b.push_block(block);
        }
    }
    if let Some((lang, text)) = b.code.take() {
        b.push_block(Block::Code { lang, text });
    }
    match b.stack.pop() {
        Some(Frame::Blocks(blocks)) => blocks,
        _ => Vec::new(),
    }
}

// ── Rendering ───────────────────────────────────────────────────────────

/// Inline spans as one wrapping text with runs. Font size is inherited.
pub fn styled_text(spans: &[Span], color: Hsla) -> StyledText {
    let mut text = String::new();
    let mut runs = Vec::with_capacity(spans.len());
    for s in spans.iter().filter(|s| !s.text.is_empty()) {
        text.push_str(&s.text);
        let mut f = font(if s.style.code { FONT_MONO } else { FONT_SANS });
        if s.style.bold {
            f.weight = FontWeight::SEMIBOLD;
        }
        if s.style.italic {
            f.style = FontStyle::Italic;
        }
        let fg = if s.style.link {
            ACCENT_TEXT
        } else if s.style.code || s.style.bold {
            TEXT
        } else {
            color
        };
        runs.push(TextRun {
            len: s.text.len(),
            font: f,
            color: fg,
            background_color: s.style.code.then_some(SURFACE_HOVER),
            underline: None,
            strikethrough: s.style.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(fg),
            }),
        });
    }
    StyledText::new(text).with_runs(runs)
}

/// A paragraph. GPUI wraps a `StyledText` using the metrics of its first font, so text
/// that mixes Inter with (wider) mono inline code overflows its column. Paragraphs with
/// inline code are therefore laid out word by word in a wrapping flex row, each word shaped
/// with its own fonts; plain paragraphs keep the cheaper single `StyledText`.
fn paragraph(spans: &[Span], color: Hsla) -> AnyElement {
    if !spans.iter().any(|s| s.style.code) {
        return div()
            .child(ui::selectable::text(styled_text(spans, color), "\n\n"))
            .into_any_element();
    }
    let mut row = div().flex().flex_wrap().items_baseline().min_w_0();
    for (i, line) in split_lines(spans).into_iter().enumerate() {
        if i > 0 {
            // Hard line break: a full-width, zero-height item.
            row = row.child(div().w_full().h(px(0.)));
        }
        for (j, word) in split_words(&line).into_iter().enumerate() {
            // Joiners rebuild the paragraph when a selection is copied.
            let joiner = match (i, j) {
                (0, 0) => "\n\n",
                (_, 0) => "\n",
                _ => "",
            };
            row = row.child(
                div()
                    .flex_none()
                    .child(ui::selectable::text(styled_text(&word, color), joiner)),
            );
        }
    }
    row.into_any_element()
}

/// Split spans at `\n` into lines (styles preserved).
fn split_lines(spans: &[Span]) -> Vec<Vec<Span>> {
    let mut lines = vec![Vec::new()];
    for s in spans {
        for (i, part) in s.text.split('\n').enumerate() {
            if i > 0 {
                lines.push(Vec::new());
            }
            if !part.is_empty() {
                lines.last_mut().unwrap().push(Span { text: part.to_string(), style: s.style });
            }
        }
    }
    lines
}

/// Split one line into words; each word keeps one trailing space so spacing matches the
/// inline layout, and may contain several styled spans (e.g. `code` followed by ".").
fn split_words(line: &[Span]) -> Vec<Vec<Span>> {
    let mut words: Vec<Vec<Span>> = vec![Vec::new()];
    for s in line {
        let mut cur = String::new();
        for ch in s.text.chars() {
            cur.push(ch);
            if ch == ' ' {
                words.last_mut().unwrap().push(Span { text: std::mem::take(&mut cur), style: s.style });
                words.push(Vec::new());
            }
        }
        if !cur.is_empty() {
            words.last_mut().unwrap().push(Span { text: cur, style: s.style });
        }
    }
    words.retain(|w| !w.is_empty());
    words
}

pub fn render(blocks: &[Block]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .min_w_0()
        .text_color(TEXT_SECONDARY)
        .children(blocks.iter().map(render_block))
        .into_any_element()
}

fn render_block(block: &Block) -> AnyElement {
    match block {
        Block::Paragraph(spans) => paragraph(spans, TEXT_SECONDARY),
        Block::Heading(level, spans) => div()
            .pt(px(if *level <= 2 { 4. } else { 2. }))
            .text_size(px(match level {
                1 => 17.,
                2 => 15.,
                _ => 13.,
            }))
            .line_height(px(if *level == 1 { 24. } else { 20. }))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(TEXT)
            .child(ui::selectable::text(styled_text(spans, TEXT), "\n\n"))
            .into_any_element(),
        Block::Code { lang, text } => code_block(lang, text).into_any_element(),
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(items.iter().enumerate().map(|(i, item)| {
                let marker = match start {
                    Some(n) => format!("{}.", n + i as u64),
                    None => "•".into(),
                };
                // Hanging marker in the left padding instead of a flex row: a flex_1 +
                // min_w_0 child of a nested flex row gets measured at zero width by taffy
                // when the item itself contains blocks (nested lists), which wrapped the
                // text one glyph per line.
                let indent = if start.is_some() { 22. } else { 16. };
                div()
                    .relative()
                    .pl(px(indent))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .text_color(TEXT_MUTED)
                            .child(marker),
                    )
                    .child(render(item))
            }))
            .into_any_element(),
        Block::Quote(blocks) => div()
            .pl(px(12.))
            .border_l_2()
            .border_color(LINE_STRONG)
            .text_color(TEXT_MUTED)
            .child(render(blocks))
            .into_any_element(),
        Block::Table { head, rows } => table(head, rows).into_any_element(),
        Block::Rule => div().h(px(1.)).bg(LINE).into_any_element(),
    }
}

/// Mono surface for fenced code; also used for raw tool output.
pub fn code_block(lang: &str, text: &str) -> gpui::Div {
    div()
        .relative()
        // Clips the selection highlight of full code lines to the block.
        .overflow_hidden()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(RADIUS_LG))
        .border_1()
        .border_color(LINE)
        .bg(BG_SIDEBAR)
        .font_family(FONT_MONO)
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(TEXT_SECONDARY)
        .when(!lang.is_empty(), |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(4.))
                    .right(px(8.))
                    .text_size(px(11.))
                    .text_color(TEXT_DISABLED)
                    .child(lang.to_string()),
            )
        })
        .child(ui::selectable::plain(text.to_string(), "\n\n"))
}

fn table(head: &[Vec<Span>], rows: &[Vec<Vec<Span>>]) -> gpui::Div {
    let row = |cells: &[Vec<Span>], header: bool| {
        div()
            .flex()
            .border_b_1()
            .border_color(LINE_SUBTLE)
            .when(header, |d| {
                d.bg(SURFACE_RAISED).font_weight(FontWeight::MEDIUM)
            })
            .children(cells.iter().map(move |c| {
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(10.))
                    .py(px(6.))
                    .child(ui::selectable::text(
                        styled_text(c, if header { TEXT } else { TEXT_SECONDARY }),
                        "\t",
                    ))
            }))
    };
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_LG))
        .overflow_hidden()
        .text_size(px(12.))
        .when(!head.is_empty(), |d| d.child(row(head, true)))
        .children(rows.iter().map(|r| row(r, false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Span {
        Span {
            text: s.into(),
            style: Style::default(),
        }
    }

    #[test]
    fn words_and_lines_split_keep_styles() {
        let blocks = parse("use `a()` now.\nnext");
        let Block::Paragraph(spans) = &blocks[0] else { panic!() };
        let lines = split_lines(spans);
        assert_eq!(lines.len(), 2);
        let words = split_words(&lines[0]);
        let texts: Vec<String> =
            words.iter().map(|w| w.iter().map(|s| s.text.as_str()).collect()).collect();
        assert_eq!(texts, ["use ", "a() ", "now."]);
        assert!(words[1][0].style.code);
    }

    #[test]
    fn paragraphs_and_inline_styles() {
        let blocks =
            parse("Hello **bold** and `code`.\nnext line\n\nSecond *it* [link](http://x).");
        assert_eq!(blocks.len(), 2);
        let Block::Paragraph(spans) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(spans[0], plain("Hello "));
        assert!(spans[1].style.bold);
        assert!(spans[3].style.code);
        assert_eq!(spans[3].text, "code");
        assert_eq!(spans[4].text, ".\nnext line");
        let Block::Paragraph(spans) = &blocks[1] else {
            panic!()
        };
        assert!(spans.iter().any(|s| s.style.italic && s.text == "it"));
        assert!(spans.iter().any(|s| s.style.link && s.text == "link"));
    }

    #[test]
    fn headings_code_and_rules() {
        let blocks = parse("# Title\n\n```rust\nfn main() {}\n```\n\n---\n## Sub");
        assert_eq!(blocks[0], Block::Heading(1, vec![plain("Title")]));
        assert_eq!(
            blocks[1],
            Block::Code {
                lang: "rust".into(),
                text: "fn main() {}".into()
            }
        );
        assert_eq!(blocks[2], Block::Rule);
        assert_eq!(blocks[3], Block::Heading(2, vec![plain("Sub")]));
    }

    #[test]
    fn unterminated_fence_while_streaming() {
        let blocks = parse("Look:\n\n```py\nprint(1)\n");
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[1],
            Block::Code {
                lang: "py".into(),
                text: "print(1)".into()
            }
        );
    }

    #[test]
    fn nested_lists() {
        let blocks = parse("1. one\n2. two\n   - a\n   - b\n3. three");
        let Block::List { start, items } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(*start, Some(1));
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], vec![Block::Paragraph(vec![plain("one")])]);
        assert_eq!(items[1].len(), 2);
        let Block::List { start, items: sub } = &items[1][1] else {
            panic!()
        };
        assert_eq!(*start, None);
        assert_eq!(sub.len(), 2);
    }

    #[test]
    fn quotes_and_tables() {
        let blocks = parse("> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(
            blocks[0],
            Block::Quote(vec![Block::Paragraph(vec![plain("quoted")])])
        );
        let Block::Table { head, rows } = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(head.len(), 2);
        assert_eq!(rows, &vec![vec![vec![plain("1")], vec![plain("2")]]]);
    }

    #[test]
    fn empty_input() {
        assert!(parse("").is_empty());
        assert!(parse("   \n\n").is_empty());
    }
}
