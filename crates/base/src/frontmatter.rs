//! The one parser for the `---` header of agent and skill Markdown files (Claude Code style).
//!
//! ```text
//! ---
//! name: reviewer
//! description: Reviews a diff
//! tools: [Read, Grep]
//! harness:
//!   kind: stage
//!   uses:
//!     - lint
//! ---
//! Body in Markdown.
//! ```
//!
//! This is a small YAML subset, not YAML: `key: value` (quotes stripped, `#` comment lines
//! ignored), inline lists `[a, b]`, block lists (`- item`), `|` and `>` block scalars, plain
//! and quoted scalars continued on deeper-indented lines, and nested maps by indentation.
//! Every scalar is a string; numbers and booleans are not interpreted. Anything the parser
//! does not understand stays a string, and nothing in it panics.

use std::collections::BTreeMap;

mod parse;
mod render;
#[cfg(test)]
mod tests;

pub use render::render;

/// A value in a header.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A scalar, or anything unrecognised kept as written.
    Str(String),
    /// An inline or block list.
    List(Vec<String>),
    /// A nested block of `key: value` lines.
    Map(BTreeMap<String, Value>),
}

/// A Markdown file split into its header and the text after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Document<'a> {
    /// The header's keys; empty when the file has no header.
    pub fields: BTreeMap<String, Value>,
    /// Everything after the closing `---` line (the whole text when there is no header).
    pub body: &'a str,
}

/// Splits `text` into header fields and body.
///
/// The header is the block between a first line of `---` and the next line of `---`. Without
/// both fences (including a header that is never closed) there is no header: the fields are
/// empty and the body is the whole text. A leading byte-order mark is ignored when looking for
/// the fence.
pub fn parse(text: &str) -> Document<'_> {
    let no_header = Document { fields: BTreeMap::new(), body: text };
    let rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = rest.split_inclusive('\n');
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return no_header,
    }
    let mut offset = rest.split_inclusive('\n').next().map_or(0, str::len);
    let mut header = Vec::new();
    for line in lines {
        offset += line.len();
        if line.trim_end() == "---" {
            return Document { fields: parse::header(&header), body: &rest[offset..] };
        }
        header.push(line);
    }
    no_header
}

impl Document<'_> {
    /// The scalar at `key`; `None` when missing or not a plain string.
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.fields.get(key) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// The list at `key`. A scalar `a, b` also gives `["a", "b"]` (Claude Code writes
    /// `tools: Read, Grep` as often as a real list); a missing key or a map gives nothing.
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.fields.get(key) {
            Some(Value::List(items)) => items.clone(),
            Some(Value::Str(s)) => {
                s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The nested block at `key`, such as `harness:`.
    pub fn map(&self, key: &str) -> Option<&BTreeMap<String, Value>> {
        match self.fields.get(key) {
            Some(Value::Map(map)) => Some(map),
            _ => None,
        }
    }
}
