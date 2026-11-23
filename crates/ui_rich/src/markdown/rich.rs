//! What a [`Document`] shows as text, without GPUI: the display string of every selectable
//! text unit (paragraph, heading, table cell, code block) with its styled spans and links,
//! the separator a copy puts before it, and the plain-text helpers built on the same walk.

use std::ops::Range;
use std::rc::Rc;

use super::parse::{Block, Document, Inline, parse};

/// The look of one span of a unit's text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
    pub mention: bool,
}

/// A styled byte range of a [`RichText`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Span {
    pub range: Range<usize>,
    pub style: SpanStyle,
}

/// Display text with contiguous spans covering it and the link targets of its link spans.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RichText {
    pub text: String,
    pub spans: Vec<Span>,
    pub links: Vec<(Range<usize>, String)>,
}

impl RichText {
    /// Unstyled text (code blocks). Tabs become four spaces: GPUI has no tab stops.
    pub fn plain(text: &str) -> Self {
        let text = text.replace('\t', "    ");
        let spans = vec![Span {
            range: 0..text.len(),
            style: SpanStyle::default(),
        }];
        Self {
            text,
            spans,
            links: Vec::new(),
        }
    }

    /// Runs of a paragraph, with `@name` for every name in `mentions` marked as a mention.
    pub fn from_inlines(runs: &[Inline], mentions: &[String]) -> Self {
        let mut out = Self::default();
        for run in runs {
            let base = SpanStyle {
                bold: run.style.bold,
                italic: run.style.italic,
                strike: run.style.strike,
                code: run.style.code,
                link: run.link.is_some(),
                mention: false,
            };
            let start = out.text.len();
            let pieces = if base.code || base.link {
                vec![(0..run.text.len(), false)]
            } else {
                split_mentions(&run.text, mentions)
            };
            for (r, mention) in pieces {
                out.push(&run.text[r], SpanStyle { mention, ..base });
            }
            if let Some(url) = &run.link {
                let end = out.text.len();
                match out.links.last_mut() {
                    Some((r, u)) if r.end == start && u == url => r.end = end,
                    _ => out.links.push((start..end, url.clone())),
                }
            }
        }
        out
    }

    fn push(&mut self, text: &str, style: SpanStyle) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        match self.spans.last_mut() {
            Some(last) if last.style == style && last.range.end == start => last.range.end = end,
            _ => self.spans.push(Span {
                range: start..end,
                style,
            }),
        }
    }

    /// The link (index into `links`) covering byte `offset`.
    pub fn link_at(&self, offset: usize) -> Option<usize> {
        self.links
            .iter()
            .position(|(r, _)| r.start <= offset && offset < r.end)
    }
}

/// Byte ranges of `text`, each flagged when it is an `@name` mention. A mention needs a
/// non-word character (or nothing) before the `@` and after the name.
pub(crate) fn split_mentions(text: &str, mentions: &[String]) -> Vec<(Range<usize>, bool)> {
    let mut out = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    while let Some(at) = text[i..].find('@').map(|p| p + i) {
        let before_ok = text[..at].chars().next_back().is_none_or(|c| !word(c));
        let rest = &text[at + 1..];
        let hit = before_ok
            .then(|| {
                mentions
                    .iter()
                    .filter(|m| !m.is_empty() && rest.starts_with(m.as_str()))
                    .filter(|m| rest[m.len()..].chars().next().is_none_or(|c| !word(c)))
                    .map(String::len)
                    .max()
            })
            .flatten();
        match hit {
            Some(len) => {
                if at > plain_start {
                    out.push((plain_start..at, false));
                }
                out.push((at..at + 1 + len, true));
                plain_start = at + 1 + len;
                i = plain_start;
            }
            None => i = at + 1,
        }
    }
    if plain_start < text.len() || out.is_empty() {
        out.push((plain_start..text.len(), false));
    }
    out
}

/// What kind of text a unit is, which decides its look.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UnitKind {
    Body,
    Heading(u8),
    Code { lang: String, raw: Rc<str> },
    HeadCell,
    Cell,
}

/// One selectable text block of a rendered document.
#[derive(Clone, Debug)]
pub(crate) struct Unit {
    pub text: Rc<RichText>,
    /// Put before this unit's text when a copied selection also covers earlier units.
    pub joiner: String,
    pub kind: UnitKind,
}

/// The selectable units of `doc` in reading order: the order the renderer walks it in.
pub(crate) fn units(doc: &Document, mentions: &[String]) -> Vec<Unit> {
    let mut w = Walk {
        mentions,
        out: Vec::new(),
    };
    for block in &doc.blocks {
        w.block(block, "\n\n");
    }
    w.out
}

struct Walk<'a> {
    mentions: &'a [String],
    out: Vec<Unit>,
}

impl Walk<'_> {
    fn unit(&mut self, text: RichText, joiner: String, kind: UnitKind) {
        self.out.push(Unit {
            text: Rc::new(text),
            joiner,
            kind,
        });
    }

    /// Emit the units of `block`; the first one gets `lead` as its joiner, later ones the
    /// separator their own structure asks for.
    fn block(&mut self, block: &Block, lead: &str) {
        match block {
            Block::Paragraph(runs) => {
                let text = RichText::from_inlines(runs, self.mentions);
                self.unit(text, lead.to_string(), UnitKind::Body);
            }
            Block::Heading { level, text } => {
                let text = RichText::from_inlines(text, self.mentions);
                self.unit(text, lead.to_string(), UnitKind::Heading(*level));
            }
            Block::Code { lang, text } => {
                let kind = UnitKind::Code {
                    lang: lang.clone(),
                    raw: Rc::from(text.as_str()),
                };
                self.unit(RichText::plain(text), lead.to_string(), kind);
            }
            Block::Quote(blocks) => {
                for (i, b) in blocks.iter().enumerate() {
                    self.block(b, if i == 0 { lead } else { "\n\n" });
                }
            }
            Block::List(list) => self.list(list, lead, 0),
            Block::Table(t) => {
                for (c, cell) in t.head.iter().enumerate() {
                    let j = if c == 0 {
                        lead.to_string()
                    } else {
                        "\t".into()
                    };
                    self.unit(
                        RichText::from_inlines(cell, self.mentions),
                        j,
                        UnitKind::HeadCell,
                    );
                }
                for (r, row) in t.rows.iter().enumerate() {
                    for (c, cell) in row.iter().enumerate() {
                        let j = match (r, c, t.head.is_empty()) {
                            (0, 0, true) => lead.to_string(),
                            (_, 0, _) => "\n".into(),
                            _ => "\t".into(),
                        };
                        let text = RichText::from_inlines(cell, self.mentions);
                        self.unit(text, j, UnitKind::Cell);
                    }
                }
            }
            Block::Rule => {}
        }
    }

    fn list(&mut self, list: &super::parse::List, lead: &str, depth: usize) {
        let indent = "  ".repeat(depth);
        for (i, item) in list.items.iter().enumerate() {
            let marker = list_marker(list.start, i, item.task);
            // The first item follows `lead`'s break; the rest start a new line.
            let brk = if i == 0 {
                lead.trim_end_matches(|c| c != '\n')
            } else {
                "\n"
            };
            let brk = if brk.is_empty() { "\n" } else { brk };
            let mut first = Some(format!("{brk}{indent}{marker} "));
            for b in &item.blocks {
                let lead = first.take().unwrap_or_else(|| format!("\n{indent}  "));
                match b {
                    Block::List(sub) => self.list(sub, &lead, depth + 1),
                    other => self.block(other, &lead),
                }
            }
        }
    }
}

/// `-`, `3.`, `- [ ]` or `- [x]`: the marker a copied list item starts with.
fn list_marker(start: Option<u64>, i: usize, task: Option<bool>) -> String {
    match (start, task) {
        (_, Some(done)) => if done { "- [x]" } else { "- [ ]" }.to_string(),
        (Some(n), None) => format!("{}.", n.saturating_add(i as u64)),
        (None, None) => "-".to_string(),
    }
}

/// The marker shown before a list item: a bullet or the item's number.
pub(crate) fn shown_marker(start: Option<u64>, i: usize) -> String {
    match start {
        Some(n) => format!("{}.", n.saturating_add(i as u64)),
        None => "•".to_string(),
    }
}

/// Markdown stripped to its text, for previews and list rows: one block per line, list
/// markers and emphasis gone, table cells separated by tabs, code kept as is.
pub fn plain_text(text: &str) -> String {
    let doc = parse(text);
    let mut out = String::new();
    for b in &doc.blocks {
        plain_block(b, &mut out);
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn plain_block(block: &Block, out: &mut String) {
    let line = |runs: &[Inline], out: &mut String| {
        for r in runs {
            out.push_str(&r.text);
        }
        out.push('\n');
    };
    match block {
        Block::Paragraph(runs) | Block::Heading { text: runs, .. } => line(runs, out),
        Block::Code { text, .. } => {
            out.push_str(text);
            out.push('\n');
        }
        Block::Quote(blocks) => blocks.iter().for_each(|b| plain_block(b, out)),
        Block::List(list) => {
            for item in &list.items {
                item.blocks.iter().for_each(|b| plain_block(b, out));
            }
        }
        Block::Table(t) => {
            for row in std::iter::once(&t.head).chain(&t.rows) {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| c.iter().map(|r| r.text.as_str()).collect())
                    .collect();
                out.push_str(&cells.join("\t"));
                out.push('\n');
            }
        }
        Block::Rule => {}
    }
}

/// The text of the first paragraph (or, failing that, the first heading), Markdown stripped
/// and line breaks turned into spaces: a one-line summary of a message.
pub fn first_paragraph(text: &str) -> String {
    fn find(blocks: &[Block], heading: bool) -> Option<&[Inline]> {
        blocks.iter().find_map(|b| match b {
            Block::Paragraph(runs) if !heading => Some(runs.as_slice()),
            Block::Heading { text, .. } if heading => Some(text.as_slice()),
            Block::Quote(bs) => find(bs, heading),
            Block::List(l) => l.items.iter().find_map(|i| find(&i.blocks, heading)),
            _ => None,
        })
    }
    let doc = parse(text);
    let runs = find(&doc.blocks, false)
        .or_else(|| find(&doc.blocks, true))
        .unwrap_or(&[]);
    let joined: String = runs.iter().map(|r| r.text.as_str()).collect();
    joined.split_whitespace().collect::<Vec<_>>().join(" ")
}
