//! Markdown source to a block tree, without GPUI.
//!
//! pulldown-cmark does the CommonMark work; the builder here folds its events into
//! [`Block`]s with flat inline runs. Any prefix of a document parses: an unclosed fence is a
//! code block up to the end, a half-written list is a list, unclosed containers are closed.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A parsed Markdown text: its top-level blocks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    /// The top-level blocks, in order.
    pub blocks: Vec<Block>,
    /// Source byte offset where each top-level block starts (same length as `blocks`).
    pub(crate) starts: Vec<usize>,
    /// The source defines link references, which can change earlier blocks; such a document
    /// is always parsed whole.
    pub(crate) has_refs: bool,
}

/// One block of a [`Document`].
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// Running text.
    Paragraph(Vec<Inline>),
    /// A heading, `level` 1 to 4 (5 and 6 are shown as 4).
    Heading {
        /// 1 to 4.
        level: u8,
        /// The heading text.
        text: Vec<Inline>,
    },
    /// A bullet or numbered list.
    List(List),
    /// A block quote.
    Quote(Vec<Block>),
    /// A fenced or indented code block.
    Code {
        /// The first word of the fence info (`rust`, `ts`), empty when there is none.
        lang: String,
        /// The code, without the trailing newline.
        text: String,
    },
    /// A GFM table.
    Table(Table),
    /// A horizontal rule.
    Rule,
}

/// A list and its items.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    /// The first number of a numbered list; `None` for bullets.
    pub start: Option<u64>,
    /// The items, in order.
    pub items: Vec<Item>,
}

/// One list item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// `Some(done)` for a task-list item (`- [ ]`, `- [x]`).
    pub task: Option<bool>,
    /// The item's content.
    pub blocks: Vec<Block>,
}

/// A table: every row has as many cells as the head.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// Column alignment, one per column.
    pub align: Vec<Align>,
    /// Header cells.
    pub head: Vec<Vec<Inline>>,
    /// Body rows of cells.
    pub rows: Vec<Vec<Vec<Inline>>>,
}

/// Horizontal alignment of a table column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Left (also when the delimiter row says nothing).
    #[default]
    Left,
    /// Centred.
    Center,
    /// Right.
    Right,
}

/// A run of text with one style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    /// The text; `\n` for a line break.
    pub text: String,
    /// How it looks.
    pub style: Style,
    /// The target URL when the run is (inside) a link.
    pub link: Option<String>,
}

/// Inline emphasis of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// `**strong**`.
    pub bold: bool,
    /// `*emphasis*`.
    pub italic: bool,
    /// `~~strikethrough~~`.
    pub strike: bool,
    /// `` `inline code` ``.
    pub code: bool,
}

/// Parse Markdown, complete or still streaming, into a [`Document`].
pub fn parse(text: &str) -> Document {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, opts).into_offset_iter();
    let has_refs = parser.reference_definitions().iter().next().is_some();
    let mut b = Builder::default();
    for (event, range) in parser {
        b.event(event, range);
    }
    let mut doc = b.finish();
    doc.has_refs = has_refs;
    doc
}

/// Blocks at the end of a streaming document that new text can still change: the last one,
/// and the one before it (a table or list can take in a line that first parsed apart).
const OPEN_BLOCKS: usize = 2;

/// Parse `text`, which is `prev_text` with more appended, reusing the blocks of `prev` that
/// the new text cannot change: all but the last [`OPEN_BLOCKS`] top-level blocks.
pub(crate) fn parse_appended(prev: &Document, prev_text: &str, text: &str) -> Document {
    let n = prev.blocks.len();
    if !text.starts_with(prev_text) || prev.has_refs || n <= OPEN_BLOCKS || prev.starts.len() != n {
        return parse(text);
    }
    let keep = n - OPEN_BLOCKS;
    // From the start of the block's line: an indented block's range starts after its indent.
    let cut = prev.starts[keep];
    let cut = prev_text
        .get(..cut)
        .and_then(|t| t.rfind('\n'))
        .map_or(0, |i| i + 1);
    let Some(tail) = text.get(cut..) else {
        return parse(text);
    };
    let tail = parse(tail);
    if tail.has_refs {
        return parse(text);
    }
    let mut blocks = prev.blocks[..keep].to_vec();
    let mut starts = prev.starts[..keep].to_vec();
    blocks.extend(tail.blocks);
    starts.extend(tail.starts.into_iter().map(|s| s + cut));
    Document {
        blocks,
        starts,
        has_refs: false,
    }
}

enum Frame {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    List(List),
    Item(Item),
}

#[derive(Default)]
struct TableState {
    align: Vec<Align>,
    head: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
    row: Vec<Vec<Inline>>,
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    starts: Vec<usize>,
    /// Source offset of the top-level block being built.
    block_start: usize,
    inline: Vec<Inline>,
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<String>,
    code: Option<(String, String)>,
    heading: Option<u8>,
    table: Option<TableState>,
}

impl Builder {
    fn at_root(&self) -> bool {
        self.stack.len() <= 1 && self.table.is_none() && self.code.is_none()
    }

    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: false,
        }
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        let link = self.links.last().cloned();
        if let Some(last) = self.inline.last_mut()
            && last.style == style
            && last.link == link
        {
            last.text.push_str(text);
            return;
        }
        self.inline.push(Inline {
            text: text.to_string(),
            style,
            link,
        });
    }

    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Root(b) | Frame::Quote(b)) => b.push(block),
            Some(Frame::Item(item)) => item.blocks.push(block),
            Some(Frame::List(list)) => list.items.push(Item {
                task: None,
                blocks: vec![block],
            }),
            None => self.stack.push(Frame::Root(vec![block])),
        }
        if self.stack.len() == 1 {
            self.starts.push(self.block_start);
        }
    }

    /// Loose inline text (a tight list item has no paragraph) becomes a paragraph.
    fn flush_inline(&mut self) {
        let mut runs = std::mem::take(&mut self.inline);
        trim_runs(&mut runs);
        if !runs.is_empty() {
            self.push_block(Block::Paragraph(runs));
        }
    }

    fn event(&mut self, event: Event, range: Range<usize>) {
        if self.stack.is_empty() {
            self.stack.push(Frame::Root(Vec::new()));
        }
        let starts_block = matches!(
            event,
            Event::Start(
                Tag::Paragraph
                    | Tag::Heading { .. }
                    | Tag::BlockQuote(_)
                    | Tag::CodeBlock(_)
                    | Tag::HtmlBlock
                    | Tag::List(_)
                    | Tag::Table(_)
            ) | Event::Rule
        );
        if starts_block && self.at_root() && self.inline.is_empty() && self.heading.is_none() {
            self.block_start = range.start;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => match self.code.as_mut() {
                Some((_, code)) => code.push_str(&t),
                None => self.push_text(&t, self.style()),
            },
            Event::Code(t) => self.push_text(
                &t,
                Style {
                    code: true,
                    ..self.style()
                },
            ),
            Event::InlineMath(t) | Event::DisplayMath(t) => self.push_text(
                &t,
                Style {
                    code: true,
                    ..self.style()
                },
            ),
            // Chat convention: a single newline is a line break, not a space.
            Event::SoftBreak | Event::HardBreak => self.push_text("\n", self.style()),
            Event::Rule => {
                self.flush_inline();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item(item)) = self.stack.last_mut() {
                    item.task = Some(done);
                }
            }
            Event::FootnoteReference(t) => self.push_text(&format!("[{t}]"), self.style()),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.flush_inline(),
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
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush_inline();
                self.stack.push(Frame::List(List {
                    start,
                    items: Vec::new(),
                }));
            }
            Tag::Item => {
                self.flush_inline();
                self.stack.push(Frame::Item(Item::default()));
            }
            Tag::Table(align) => {
                self.flush_inline();
                let align = align.into_iter().map(align_of).collect();
                self.table = Some(TableState {
                    align,
                    ..Default::default()
                });
            }
            Tag::TableHead | Tag::TableRow | Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links.push(dest_url.to_string())
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.flush_inline(),
            TagEnd::Heading(_) => {
                let mut runs = std::mem::take(&mut self.inline);
                trim_runs(&mut runs);
                let level = self.heading.take().unwrap_or(4);
                if !runs.is_empty() {
                    self.push_block(Block::Heading { level, text: runs });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                if let Some(Frame::Quote(blocks)) = self.pop_frame() {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, mut text)) = self.code.take() {
                    trim_code(&mut text);
                    self.push_block(Block::Code { lang, text });
                }
            }
            TagEnd::List(_) => {
                self.flush_inline();
                if let Some(Frame::List(list)) = self.pop_frame() {
                    self.push_block(Block::List(list));
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                if let Some(Frame::Item(item)) = self.pop_frame() {
                    self.push_item(item);
                }
            }
            TagEnd::TableCell => {
                let mut cell = std::mem::take(&mut self.inline);
                trim_runs(&mut cell);
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
                    self.push_block(Block::Table(table_of(t)));
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            _ => {}
        }
    }

    /// Pop a frame, never the root.
    fn pop_frame(&mut self) -> Option<Frame> {
        if self.stack.len() > 1 {
            self.stack.pop()
        } else {
            None
        }
    }

    fn push_item(&mut self, item: Item) {
        match self.stack.last_mut() {
            Some(Frame::List(list)) => list.items.push(item),
            _ => item.blocks.into_iter().for_each(|b| self.push_block(b)),
        }
    }

    /// Close whatever the (possibly unfinished) source left open and return the document.
    fn finish(mut self) -> Document {
        if let Some((lang, mut text)) = self.code.take() {
            trim_code(&mut text);
            self.push_block(Block::Code { lang, text });
        }
        if let Some(mut t) = self.table.take() {
            if !t.row.is_empty() {
                let row = std::mem::take(&mut t.row);
                t.rows.push(row);
            }
            self.push_block(Block::Table(table_of(t)));
        }
        if let Some(level) = self.heading.take() {
            let mut runs = std::mem::take(&mut self.inline);
            trim_runs(&mut runs);
            if !runs.is_empty() {
                self.push_block(Block::Heading { level, text: runs });
            }
        }
        self.flush_inline();
        while self.stack.len() > 1 {
            match self.stack.pop() {
                Some(Frame::Quote(blocks)) => self.push_block(Block::Quote(blocks)),
                Some(Frame::List(list)) => self.push_block(Block::List(list)),
                Some(Frame::Item(item)) => self.push_item(item),
                Some(Frame::Root(blocks)) => blocks.into_iter().for_each(|b| self.push_block(b)),
                None => {}
            }
        }
        let blocks = match self.stack.pop() {
            Some(Frame::Root(blocks)) => blocks,
            _ => Vec::new(),
        };
        let mut starts = self.starts;
        starts.resize(blocks.len(), self.block_start);
        Document {
            blocks,
            starts,
            has_refs: false,
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        _ => 4,
    }
}

fn align_of(a: Alignment) -> Align {
    match a {
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
        Alignment::Left | Alignment::None => Align::Left,
    }
}

/// Every row as wide as the head (padded with empty cells or cut).
fn table_of(t: TableState) -> Table {
    let cols = if t.head.is_empty() {
        t.rows.iter().map(Vec::len).max().unwrap_or(0)
    } else {
        t.head.len()
    };
    let mut head = t.head;
    head.resize(cols, Vec::new());
    let rows = t
        .rows
        .into_iter()
        .map(|mut r| {
            r.resize(cols, Vec::new());
            r
        })
        .collect();
    let mut align = t.align;
    align.resize(cols, Align::Left);
    Table { align, head, rows }
}

/// Drop trailing whitespace and empty runs.
fn trim_runs(runs: &mut Vec<Inline>) {
    while let Some(last) = runs.last_mut() {
        let keep = last.text.trim_end().len();
        last.text.truncate(keep);
        if last.text.is_empty() {
            runs.pop();
        } else {
            break;
        }
    }
    runs.retain(|r| !r.text.is_empty());
}

fn trim_code(text: &mut String) {
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
}
