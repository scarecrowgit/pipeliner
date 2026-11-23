//! Writing a header back out: the inverse of [`parse`](super::parse).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::Value;

/// Longest line an inline list `[a, b]` may take before it is written as a block list.
const INLINE_LIST_MAX: usize = 80;

/// The text of a file with `fields` as its header and `body` after it.
///
/// Keys come out in sorted order, so output is stable. `parse(&render(f, b))` gives back
/// `f` and `b`, with three limits: a scalar's trailing line breaks are not kept, empty list
/// items are dropped, and keys must be plain words (no `:`). With no fields and a body that does not itself start with a `---`
/// line, no header is written at all.
pub fn render(fields: &BTreeMap<String, Value>, body: &str) -> String {
    let starts_with_fence = body.lines().next().is_some_and(|l| l.trim_end() == "---");
    if fields.is_empty() && !starts_with_fence {
        return body.to_string();
    }
    let mut out = String::from("---\n");
    write_map(&mut out, fields, 0);
    out.push_str("---\n");
    out.push_str(body);
    out
}

fn write_map(out: &mut String, map: &BTreeMap<String, Value>, indent: usize) {
    let pad = " ".repeat(indent);
    for (key, value) in map {
        match value {
            Value::Str(s) if s.contains('\n') => write_block(out, key, s, indent),
            Value::Str(s) => {
                let _ = writeln!(out, "{pad}{key}: {}", scalar(s));
            }
            Value::List(items) => write_list(out, key, items, indent),
            Value::Map(inner) if inner.is_empty() => {
                let _ = writeln!(out, "{pad}{key}: {{}}");
            }
            Value::Map(inner) => {
                let _ = writeln!(out, "{pad}{key}:");
                write_map(out, inner, indent + 2);
            }
        }
    }
}

/// A multi-line string as a `|` block, indented two columns under its key.
fn write_block(out: &mut String, key: &str, text: &str, indent: usize) {
    let pad = " ".repeat(indent);
    // Content that starts with a space needs the indentation spelled out.
    let leading_space = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| l.starts_with([' ', '\t']));
    let _ = writeln!(out, "{pad}{key}: |{}", if leading_space { "2" } else { "" });
    for line in text.trim_end_matches('\n').split('\n') {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{pad}  {line}");
        }
    }
}

fn write_list(out: &mut String, key: &str, items: &[String], indent: usize) {
    let pad = " ".repeat(indent);
    if items.is_empty() {
        let _ = writeln!(out, "{pad}{key}: []");
        return;
    }
    if items.iter().all(|i| is_inline_safe(i)) {
        let line = format!("{pad}{key}: [{}]", items.join(", "));
        if line.len() <= INLINE_LIST_MAX {
            out.push_str(&line);
            out.push('\n');
            return;
        }
    }
    let _ = writeln!(out, "{pad}{key}:");
    for item in items {
        let _ = writeln!(out, "{pad}  - {}", scalar(item));
    }
}

/// Whether an item can sit in `[a, b]` as it is.
fn is_inline_safe(item: &str) -> bool {
    !needs_quotes(item) && !item.contains([',', '[', ']', '"', '\''])
}

/// A single-line value, quoted when reading it back would otherwise change it.
fn scalar(text: &str) -> String {
    if needs_quotes(text) { quote(text) } else { text.to_string() }
}

fn needs_quotes(text: &str) -> bool {
    text.is_empty()
        || text.trim() != text
        || text.starts_with(['[', '{', '"', '\'', '|', '>', '#', '&', '*', '!', '%', '@', '`', '?'])
        || text == "-"
        || text.starts_with("- ")
        || text.contains(": ")
        || text.contains(" #")
        || text.ends_with(':')
        || text.contains(['\n', '\r', '\t'])
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
