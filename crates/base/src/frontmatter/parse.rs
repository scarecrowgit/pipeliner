//! The header parser: lines to a tree of [`Value`]s.

use std::collections::BTreeMap;

use super::Value;

/// How many levels of nested maps are read; deeper lines are skipped.
const MAX_DEPTH: usize = 6;

/// One header line with its indentation worked out.
struct Line<'a> {
    /// The line without its line break and trailing spaces.
    raw: &'a str,
    /// Leading spaces and tabs.
    indent: usize,
    /// The line after the indentation.
    text: &'a str,
}

impl<'a> Line<'a> {
    fn new(line: &'a str) -> Self {
        let raw = line.trim_end();
        let text = raw.trim_start_matches([' ', '\t']);
        Line { raw, indent: raw.len() - text.len(), text }
    }

    fn is_trivia(&self) -> bool {
        self.text.is_empty() || self.text.starts_with('#')
    }
}

/// Parses the lines between the fences into the top-level fields.
pub(super) fn header(lines: &[&str]) -> BTreeMap<String, Value> {
    let mut parser = Parser { lines: lines.iter().map(|l| Line::new(l)).collect(), pos: 0 };
    parser.map(0)
}

struct Parser<'a> {
    lines: Vec<Line<'a>>,
    pos: usize,
}

/// The header of a `|` or `>` block scalar.
struct BlockHeader {
    fold: bool,
    /// An explicit indentation indicator: content sits this far right of the key.
    indent: Option<usize>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Line<'a>> {
        self.lines.get(self.pos)
    }

    fn skip_trivia(&mut self) {
        while self.peek().is_some_and(Line::is_trivia) {
            self.pos += 1;
        }
    }

    /// Reads `key: value` entries that start at the indentation of the next entry and stop at
    /// the first line indented less.
    fn map(&mut self, depth: usize) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        self.skip_trivia();
        let Some(indent) = self.peek().map(|l| l.indent) else { return out };
        loop {
            self.skip_trivia();
            let Some(line) = self.peek() else { break };
            if line.indent < indent {
                break;
            }
            let (key_indent, text) = (line.indent, line.text);
            self.pos += 1;
            // A line that is not `key: value` has nowhere to go; skip it.
            let Some((key, rest)) = split_key(text) else { continue };
            let value = self.value(key_indent, rest, depth);
            out.insert(key.to_string(), value);
        }
        out
    }

    /// The value of a key whose line had `rest` after the colon.
    fn value(&mut self, key_indent: usize, rest: &str, depth: usize) -> Value {
        if rest.is_empty() {
            return self.nested(key_indent, depth);
        }
        if let Some(block) = block_header(rest) {
            return Value::Str(self.block_scalar(key_indent, &block));
        }
        if rest.starts_with('[') {
            return self.inline_list(key_indent, rest);
        }
        if rest == "{}" {
            return Value::Map(BTreeMap::new());
        }
        let mut text = rest.to_string();
        self.continue_scalar(key_indent, &mut text);
        Value::Str(unquote(&text))
    }

    /// Appends the lines under a scalar that are indented deeper than its key.
    fn continue_scalar(&mut self, key_indent: usize, text: &mut String) {
        while let Some(line) = self.peek() {
            if line.is_trivia() || line.indent <= key_indent {
                break;
            }
            text.push(' ');
            text.push_str(line.text);
            self.pos += 1;
        }
    }

    /// The value of `key:` with nothing after the colon: a block list, a nested map, a scalar
    /// on the following lines, or an empty string.
    fn nested(&mut self, key_indent: usize, depth: usize) -> Value {
        self.skip_trivia();
        let Some(next) = self.peek() else { return Value::Str(String::new()) };
        let (indent, text) = (next.indent, next.text);
        if is_item(text) && indent >= key_indent {
            return Value::List(self.block_list(indent));
        }
        if indent <= key_indent {
            return Value::Str(String::new());
        }
        if split_key(text).is_none() {
            // Not a map: the value is plain text starting on the next line.
            let mut value = String::new();
            while let Some(line) = self.peek() {
                if line.is_trivia() || line.indent <= key_indent {
                    break;
                }
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(line.text);
                self.pos += 1;
            }
            return Value::Str(unquote(&value));
        }
        if depth >= MAX_DEPTH {
            self.skip_deeper(key_indent);
            return Value::Str(String::new());
        }
        Value::Map(self.map(depth + 1))
    }

    fn skip_deeper(&mut self, key_indent: usize) {
        while self.peek().is_some_and(|l| l.is_trivia() || l.indent > key_indent) {
            self.pos += 1;
        }
    }

    /// `- item` lines at `indent`.
    fn block_list(&mut self, indent: usize) -> Vec<String> {
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            let Some(line) = self.peek() else { break };
            if line.indent < indent || !is_item(line.text) {
                break;
            }
            let item = unquote(line.text[1..].trim());
            self.pos += 1;
            if !item.is_empty() {
                items.push(item);
            }
        }
        items
    }

    /// `[a, b]`, possibly continued on the following lines. Without a closing bracket the line
    /// stays a string.
    fn inline_list(&mut self, key_indent: usize, rest: &str) -> Value {
        let mut text = rest.to_string();
        let mut end = self.pos;
        while !text.ends_with(']') {
            match self.lines.get(end) {
                Some(line) if !line.is_trivia() && line.indent > key_indent => {
                    text.push(' ');
                    text.push_str(line.text);
                    end += 1;
                }
                _ => return Value::Str(rest.to_string()),
            }
        }
        self.pos = end;
        Value::List(split_inline(&text[1..text.len() - 1]))
    }

    /// The text of a `|` or `>` block: every following line that is blank or indented deeper
    /// than the key, with the common indentation removed and trailing blank lines dropped.
    fn block_scalar(&mut self, key_indent: usize, header: &BlockHeader) -> String {
        let mut taken: Vec<&Line<'a>> = Vec::new();
        let mut end = self.pos;
        while let Some(line) = self.lines.get(end) {
            if !line.text.is_empty() && line.indent <= key_indent {
                break;
            }
            taken.push(line);
            end += 1;
        }
        self.pos = end;
        let content = header
            .indent
            .map(|n| key_indent + n)
            .or_else(|| taken.iter().find(|l| !l.text.is_empty()).map(|l| l.indent))
            .unwrap_or(0);
        let mut lines: Vec<&str> = taken
            .iter()
            .map(|l| if l.text.is_empty() { "" } else { &l.raw[l.indent.min(content)..] })
            .collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        if header.fold { fold(&lines) } else { lines.join("\n") }
    }
}

/// Splits `key: rest` at the first colon that is followed by a space or the end of the line.
fn split_key(text: &str) -> Option<(&str, &str)> {
    if is_item(text) {
        return None;
    }
    let mut from = 0;
    while let Some(at) = text[from..].find(':') {
        let colon = from + at;
        let after = &text[colon + 1..];
        if after.is_empty() || after.starts_with([' ', '\t']) {
            let key = strip_quotes(text[..colon].trim());
            return (!key.is_empty()).then_some((key, after.trim()));
        }
        from = colon + 1;
    }
    None
}

fn is_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// Recognises `|`, `>`, `|-`, `>+`, `|2` and similar.
fn block_header(rest: &str) -> Option<BlockHeader> {
    let mut chars = rest.chars();
    let fold = match chars.next()? {
        '|' => false,
        '>' => true,
        _ => return None,
    };
    let mut indent = None;
    for c in chars {
        match c {
            '+' | '-' => {}
            '1'..='9' => indent = c.to_digit(10).map(|d| d as usize),
            _ => return None,
        }
    }
    Some(BlockHeader { fold, indent })
}

/// Folds lines of a `>` scalar: line breaks become spaces, a blank line becomes a line break.
fn fold(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut fresh = true;
    for line in lines {
        if line.is_empty() {
            out.push('\n');
            fresh = true;
        } else {
            if !fresh {
                out.push(' ');
            }
            out.push_str(line);
            fresh = false;
        }
    }
    out
}

/// The items of an inline list body, split at commas outside quotes.
fn split_inline(inner: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in inner.chars() {
        match quote {
            Some(q) => {
                current.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if (c == '"' || c == '\'') && current.trim().is_empty() => {
                quote = Some(c);
                current.push(c);
            }
            None if c == ',' => items.push(std::mem::take(&mut current)),
            None => current.push(c),
        }
    }
    items.push(current);
    items.iter().map(|i| unquote(i.trim())).filter(|i| !i.is_empty()).collect()
}

/// The text inside one pair of matching quotes, or the text unchanged.
fn strip_quotes(text: &str) -> &str {
    let bytes = text.as_bytes();
    let quoted = bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0];
    if quoted { &text[1..text.len() - 1] } else { text }
}

/// Strips one pair of matching quotes and resolves their escapes.
pub(super) fn unquote(text: &str) -> String {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        return unescape(&text[1..text.len() - 1]);
    }
    if bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        return text[1..text.len() - 1].replace("''", "'");
    }
    text.to_string()
}

fn unescape(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(e @ ('"' | '\\' | '/')) => out.push(e),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
