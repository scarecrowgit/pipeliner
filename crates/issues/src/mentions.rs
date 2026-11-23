//! `@Name` mentions in comments: finding them, completing them while typing.

use std::ops::Range;

use model::{IssueKey, Runnable, RunnableKind, RunnableRef};

#[cfg(test)]
mod tests;

/// A longest prefix of a prefix typed after `@` that the composer looks at, in characters.
const MAX_PREFIX_CHARS: usize = 40;

/// One `@Name` in a text that names a known runnable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// Byte range in the text, `@` included.
    pub range: Range<usize>,
    pub target: RunnableRef,
    /// The runnable's name as it is spelled in the library (not as typed).
    pub name: String,
}

/// Finds the mentions of known runnables in `text`.
///
/// Matching is case-insensitive and longest name first, so `@Release captain` is not read as
/// `@Release`. A mention must start a word (`bob@example.com` has none), must end at a word
/// boundary (`@Fixers` is not `@Fixer`) and is ignored inside code spans.
pub fn find_mentions(text: &str, runnables: &[Runnable]) -> Vec<Mention> {
    let mut names: Vec<(&Runnable, &str)> =
        runnables.iter().map(|r| (r, r.name.trim())).filter(|(_, name)| !name.is_empty()).collect();
    names.sort_by_key(|(_, name)| std::cmp::Reverse(name.chars().count()));
    let code = code_spans(text);

    let mut found = Vec::new();
    let mut prev: Option<char> = None;
    let mut resume = 0;
    for (at, c) in text.char_indices() {
        let skip = at < resume;
        if !skip && c == '@' && starts_word(prev) && !in_code(&code, at) {
            let rest = &text[at + 1..];
            let hit = names.iter().find_map(|(r, name)| {
                let len = match_prefix_ci(rest, name)?;
                let boundary = rest[len..].chars().next().is_none_or(|n| !is_name_char(n));
                boundary.then_some((*r, len))
            });
            if let Some((r, len)) = hit {
                resume = at + 1 + len;
                found.push(Mention { range: at..resume, target: r.reference.clone(), name: r.name.clone() });
            }
        }
        prev = Some(c);
    }
    found
}

/// The distinct runnables a list of mentions names, in order of first appearance.
pub fn targets(mentions: &[Mention]) -> Vec<RunnableRef> {
    let mut out: Vec<RunnableRef> = Vec::new();
    for m in mentions {
        if !out.contains(&m.target) {
            out.push(m.target.clone());
        }
    }
    out
}

/// How an entry of the autocomplete popup relates to the issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The runner attached to this issue: continues with the comment as input.
    Attached,
    /// Any other runnable: starts a new run on the issue.
    NewRun,
    /// Paused: cannot be woken.
    Paused,
}

/// One row of the autocomplete popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub target: RunnableRef,
    pub name: String,
    pub kind: RunnableKind,
    pub standing: Standing,
    /// Byte range of `name` that matched what was typed, to draw in the accent colour.
    pub highlight: Range<usize>,
    /// "Agent · on this issue now · gets your comment as input".
    pub description: String,
}

impl CompletionItem {
    /// The text inserted when the row is picked: `@Release captain `.
    pub fn insert_text(&self) -> String {
        format!("@{} ", self.name)
    }

    /// The description with the issue named: "Orchestrator · starts a new run on PAY-1427".
    pub fn description_on(&self, issue: &IssueKey) -> String {
        describe(self.kind, self.standing, issue.as_str())
    }
}

/// The popup's content: the rows to show and how many matched in all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub prefix: String,
    pub items: Vec<CompletionItem>,
    /// Matches before the limit cut the list.
    pub total: usize,
}

impl Completion {
    /// The popup's label: `Matching “re” · 2 of 8`.
    pub fn heading(&self) -> String {
        let (shown, total) = (self.items.len(), self.total);
        if self.prefix.is_empty() {
            format!("Agents and pipelines · {shown} of {total}")
        } else {
            format!("Matching “{}” · {shown} of {total}", self.prefix)
        }
    }
}

/// The runnables that match what was typed after `@`.
///
/// Archived runnables and those not offered in issues are left out. Order: names starting with
/// the prefix, then names containing it; within each, the attached runner first, then
/// orchestrators, agents and pipelines, then by name. At most `limit` rows come back, with
/// `total` counting every match.
pub fn complete(prefix: &str, runnables: &[Runnable], attached: Option<&RunnableRef>, limit: usize) -> Completion {
    let mut hits: Vec<(u8, Range<usize>, &Runnable)> = Vec::new();
    for r in runnables.iter().filter(|r| !r.archived && r.uses.issues) {
        if prefix.is_empty() {
            hits.push((0, 0..0, r));
        } else if let Some(len) = match_prefix_ci(&r.name, prefix) {
            hits.push((0, 0..len, r));
        } else if let Some(range) = r.name.char_indices().find_map(|(i, _)| {
            match_prefix_ci(&r.name[i..], prefix).map(|len| i..i + len)
        }) {
            hits.push((1, range, r));
        }
    }
    hits.sort_by_cached_key(|(class, _, r)| {
        (*class, attached != Some(&r.reference), kind_rank(r.kind), r.name.to_lowercase(), r.reference.clone())
    });
    let total = hits.len();
    let items = hits
        .into_iter()
        .take(limit)
        .map(|(_, highlight, r)| {
            let standing = if r.paused {
                Standing::Paused
            } else if attached == Some(&r.reference) {
                Standing::Attached
            } else {
                Standing::NewRun
            };
            CompletionItem {
                target: r.reference.clone(),
                name: r.name.clone(),
                kind: r.kind,
                standing,
                highlight,
                description: describe(r.kind, standing, "this issue"),
            }
        })
        .collect();
    Completion { prefix: prefix.to_string(), items, total }
}

/// The mention being typed at `cursor` (a byte offset): where its `@` is and what follows it up
/// to the cursor. `None` when the cursor is not right after an `@` that starts a word outside a
/// code span, or when something that cannot be in a name (punctuation, a line break) sits
/// between. Spaces are kept so `@Release c` completes multi-word names; an empty result list
/// closes the popup once the name is complete.
pub fn mention_at_cursor(text: &str, cursor: usize) -> Option<(usize, String)> {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let before = &text[..cursor];
    let mut chars = 0;
    for (at, c) in before.char_indices().rev() {
        if c == '@' {
            let starts = starts_word(before[..at].chars().next_back());
            return (starts && !in_code(&code_spans(text), at)).then(|| (at, before[at + 1..].to_string()));
        }
        chars += 1;
        if chars > MAX_PREFIX_CHARS || !(is_name_char(c) || c == ' ' || c == '-') {
            return None;
        }
    }
    None
}

fn describe(kind: RunnableKind, standing: Standing, issue: &str) -> String {
    let kind = match kind {
        RunnableKind::Agent => "Agent",
        RunnableKind::Orchestrator => "Orchestrator",
        RunnableKind::Pipeline => "Pipeline",
    };
    match standing {
        Standing::Attached => format!("{kind} · on this issue now · gets your comment as input"),
        Standing::NewRun => format!("{kind} · starts a new run on {issue}"),
        Standing::Paused => format!("{kind} · paused · takes no new work"),
    }
}

fn kind_rank(kind: RunnableKind) -> u8 {
    match kind {
        RunnableKind::Orchestrator => 0,
        RunnableKind::Agent => 1,
        RunnableKind::Pipeline => 2,
    }
}

/// A character that continues a word: a name does not end right before one.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// An `@` after this character (or at the start) opens a mention; after a letter or digit it is
/// an address.
fn starts_word(prev: Option<char>) -> bool {
    prev.is_none_or(|p| !is_name_char(p) && p != '@')
}

/// Length in bytes of the start of `hay` that equals `needle` ignoring case.
fn match_prefix_ci(hay: &str, needle: &str) -> Option<usize> {
    let mut end = 0;
    let mut chars = hay.chars();
    for n in needle.chars() {
        let h = chars.next()?;
        if h != n && !h.to_lowercase().eq(n.to_lowercase()) {
            return None;
        }
        end += h.len_utf8();
    }
    Some(end)
}

/// Byte ranges of inline code spans and fenced blocks: a run of backticks up to the next run of
/// the same length. An opening run with no closing one is plain text.
fn code_spans(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let run_at = |from: usize| bytes[from..].iter().take_while(|b| **b == b'`').count();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let open = run_at(i);
        let mut j = i + open;
        let mut closed = None;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                let close = run_at(j);
                if close == open {
                    closed = Some(j + close);
                    break;
                }
                j += close;
            } else {
                j += 1;
            }
        }
        match closed {
            Some(end) => {
                spans.push(i..end);
                i = end;
            }
            None => i += open,
        }
    }
    spans
}

fn in_code(spans: &[Range<usize>], at: usize) -> bool {
    spans.iter().any(|s| s.contains(&at))
}
