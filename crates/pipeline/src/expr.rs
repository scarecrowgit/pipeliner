//! Router conditions: the language of [`model::RouterRule::when`].
//!
//! ```text
//! type == "bug" && priority >= "high"
//! title contains "crash" || text matches "*panic*"
//! !(status == "done")
//! ```
//!
//! Grammar, loosest binding first:
//!
//! ```text
//! expr    := and ( ("||" | "or") and )*
//! and     := unary ( ("&&" | "and") unary )*
//! unary   := ("!" | "not") unary | "(" expr ")" | compare
//! compare := field op "quoted value"
//! op      := == | != | < | <= | > | >= | contains | matches
//! ```
//!
//! Fields are listed in [`FIELDS`]. Comparisons ignore case. `priority` and `status` are
//! ordered (`low < medium < high`, `backlog < todo < in_progress < in_review < done`) and are
//! the only fields `<`, `<=`, `>`, `>=` accept. `contains` is a substring test, `matches` a glob
//! (`*`, `?`, `[abc]`). Values of `type`, `priority`, `status` and `source` are checked when
//! the condition is parsed. When the item has no issue, issue fields are empty: every test on
//! them is false except `!=`.

#[cfg(test)]
mod tests;

use std::fmt;

use globset::{GlobBuilder, GlobMatcher};
use model::{IssueStatus, IssueType, Priority, SourceKind};

use crate::api::Item;

/// Field names a condition may use, as the error for an unknown one lists them.
pub const FIELDS: &[&str] = &["type", "priority", "status", "title", "text", "body", "owner", "assignee", "key", "repo", "source"];

const PRIORITIES: &[(&str, Priority)] = &[("low", Priority::Low), ("medium", Priority::Medium), ("high", Priority::High)];
const TYPES: &[(&str, IssueType)] =
    &[("bug", IssueType::Bug), ("story", IssueType::Story), ("chore", IssueType::Chore), ("docs", IssueType::Docs)];
const STATUSES: &[(&str, IssueStatus)] = &[
    ("backlog", IssueStatus::Backlog),
    ("todo", IssueStatus::Todo),
    ("in_progress", IssueStatus::InProgress),
    ("in_review", IssueStatus::InReview),
    ("done", IssueStatus::Done),
];
const SOURCES: &[(&str, SourceKind)] = &[
    ("issue", SourceKind::Issue),
    ("manual", SourceKind::Manual),
    ("schedule", SourceKind::Schedule),
    ("jira", SourceKind::Jira),
    ("sentry", SourceKind::Sentry),
];

/// Why a condition did not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExprError {
    /// Byte offset into the condition where the problem starts.
    pub pos: usize,
    pub message: String,
}

impl ExprError {
    fn new(pos: usize, message: impl Into<String>) -> Self {
        Self { pos, message: message.into() }
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at {})", self.message, self.pos + 1)
    }
}

impl std::error::Error for ExprError {}

/// A field of the item a condition reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Type,
    Priority,
    Status,
    Title,
    Text,
    Owner,
    Key,
    Repo,
    Source,
}

impl Field {
    fn parse(name: &str) -> Option<Field> {
        Some(match name {
            "type" => Field::Type,
            "priority" => Field::Priority,
            "status" => Field::Status,
            "title" => Field::Title,
            "text" | "body" => Field::Text,
            "owner" | "assignee" => Field::Owner,
            "key" => Field::Key,
            "repo" => Field::Repo,
            "source" => Field::Source,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Field::Type => "type",
            Field::Priority => "priority",
            Field::Status => "status",
            Field::Title => "title",
            Field::Text => "text",
            Field::Owner => "owner",
            Field::Key => "key",
            Field::Repo => "repo",
            Field::Source => "source",
        }
    }

    /// The fixed set of values the field takes, in order, when it has one.
    fn values(self) -> Option<Vec<&'static str>> {
        match self {
            Field::Type => Some(names(TYPES)),
            Field::Priority => Some(names(PRIORITIES)),
            Field::Status => Some(names(STATUSES)),
            Field::Source => Some(names(SOURCES)),
            _ => None,
        }
    }

    fn ordered(self) -> bool {
        matches!(self, Field::Priority | Field::Status)
    }

    /// The field's value on `item` as text, and its rank for ordered fields. `None` when the
    /// item has no issue and the field belongs to it.
    fn read(self, item: &Item) -> Option<(String, usize)> {
        let issue = item.issue.as_ref();
        match self {
            Field::Title => Some((item.title.clone(), 0)),
            Field::Text => Some((item.text.clone(), 0)),
            Field::Source => rank(SOURCES, item.source),
            Field::Type => rank(TYPES, issue?.kind),
            Field::Priority => rank(PRIORITIES, issue?.priority),
            Field::Status => rank(STATUSES, issue?.status),
            Field::Owner => Some((issue?.owner.clone(), 0)),
            Field::Key => Some((issue?.key.to_string(), 0)),
            Field::Repo => Some((issue?.repo.clone().unwrap_or_default(), 0)),
        }
    }
}

fn names<T>(table: &[(&'static str, T)]) -> Vec<&'static str> {
    table.iter().map(|(name, _)| *name).collect()
}

fn rank<T: PartialEq>(table: &[(&'static str, T)], value: T) -> Option<(String, usize)> {
    table.iter().position(|(_, v)| *v == value).map(|i| (table[i].0.to_string(), i))
}

/// How a field is compared with a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    Matches,
}

impl Op {
    fn word(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Contains => "contains",
            Op::Matches => "matches",
        }
    }
}

/// A compiled glob of `matches`; equal when the pattern text is.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub text: String,
    matcher: GlobMatcher,
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

/// The right-hand side of a comparison.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Lowercased text.
    Text(String),
    /// Position of an ordered field's value.
    Rank(usize),
    Glob(Pattern),
}

/// A parsed condition.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Compare { field: Field, op: Op, value: Value },
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Whether `item` satisfies the condition.
    pub fn eval(&self, item: &Item) -> bool {
        match self {
            Expr::Not(inner) => !inner.eval(item),
            Expr::And(a, b) => a.eval(item) && b.eval(item),
            Expr::Or(a, b) => a.eval(item) || b.eval(item),
            Expr::Compare { field, op, value } => {
                let Some((text, rank)) = field.read(item) else {
                    return *op == Op::Ne;
                };
                let text = text.to_lowercase();
                match (op, value) {
                    (Op::Matches, Value::Glob(pattern)) => pattern.matcher.is_match(&text),
                    (Op::Contains, Value::Text(want)) => text.contains(want.as_str()),
                    (Op::Eq, Value::Text(want)) => &text == want,
                    (Op::Ne, Value::Text(want)) => &text != want,
                    (op, Value::Rank(want)) => match op {
                        Op::Eq => rank == *want,
                        Op::Ne => rank != *want,
                        Op::Lt => rank < *want,
                        Op::Le => rank <= *want,
                        Op::Gt => rank > *want,
                        Op::Ge => rank >= *want,
                        Op::Contains | Op::Matches => false,
                    },
                    _ => false,
                }
            }
        }
    }
}

/// Parses a condition. Unknown fields, unknown values of fixed fields and misplaced tokens
/// are errors with the position of the offending token.
pub fn parse(text: &str) -> Result<Expr, ExprError> {
    let tokens = tokenize(text)?;
    if tokens.is_empty() {
        return Err(ExprError::new(0, "The condition is empty"));
    }
    let mut parser = Parser { tokens, at: 0, end: text.len() };
    let expr = parser.or()?;
    match parser.peek() {
        None => Ok(expr),
        Some((tok, pos)) => Err(ExprError::new(pos, format!("Expected && or || before {}", tok.describe()))),
    }
}

// ---------------------------------------------------------------- tokens

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Op(Op),
    And,
    Or,
    Not,
    Open,
    Close,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Word(word) => format!("“{word}”"),
            Tok::Str(text) => format!("\"{text}\""),
            Tok::Op(op) => format!("“{}”", op.word()),
            Tok::And => "“&&”".into(),
            Tok::Or => "“||”".into(),
            Tok::Not => "“!”".into(),
            Tok::Open => "“(”".into(),
            Tok::Close => "“)”".into(),
        }
    }
}

fn tokenize(text: &str) -> Result<Vec<(Tok, usize)>, ExprError> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(pos, c)) = chars.peek() {
        chars.next();
        let next_is = |chars: &mut std::iter::Peekable<std::str::CharIndices>, want: char| {
            if chars.peek().map(|&(_, c)| c) == Some(want) {
                chars.next();
                true
            } else {
                false
            }
        };
        let tok = match c {
            c if c.is_whitespace() => continue,
            '(' => Tok::Open,
            ')' => Tok::Close,
            '&' if next_is(&mut chars, '&') => Tok::And,
            '|' if next_is(&mut chars, '|') => Tok::Or,
            '=' if next_is(&mut chars, '=') => Tok::Op(Op::Eq),
            '!' if next_is(&mut chars, '=') => Tok::Op(Op::Ne),
            '!' => Tok::Not,
            '<' if next_is(&mut chars, '=') => Tok::Op(Op::Le),
            '<' => Tok::Op(Op::Lt),
            '>' if next_is(&mut chars, '=') => Tok::Op(Op::Ge),
            '>' => Tok::Op(Op::Gt),
            '"' | '\'' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some((_, ch)) = chars.next() {
                    match ch {
                        '\\' => match chars.next() {
                            Some((_, escaped)) => value.push(escaped),
                            None => break,
                        },
                        ch if ch == c => {
                            closed = true;
                            break;
                        }
                        ch => value.push(ch),
                    }
                }
                if !closed {
                    return Err(ExprError::new(pos, "The quoted value is not closed"));
                }
                Tok::Str(value)
            }
            c if c.is_alphanumeric() || c == '_' => {
                let mut word = c.to_string();
                while let Some(&(_, ch)) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                        word.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match word.as_str() {
                    "and" => Tok::And,
                    "or" => Tok::Or,
                    "not" => Tok::Not,
                    "contains" => Tok::Op(Op::Contains),
                    "matches" => Tok::Op(Op::Matches),
                    _ => Tok::Word(word),
                }
            }
            '=' => return Err(ExprError::new(pos, "Use == to compare")),
            '&' => return Err(ExprError::new(pos, "Use && for “and”")),
            '|' => return Err(ExprError::new(pos, "Use || for “or”")),
            other => return Err(ExprError::new(pos, format!("Unexpected “{other}”"))),
        };
        out.push((tok, pos));
    }
    Ok(out)
}

// ---------------------------------------------------------------- parser

struct Parser {
    tokens: Vec<(Tok, usize)>,
    at: usize,
    /// Byte length of the text, the position of "the end".
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<(&Tok, usize)> {
        self.tokens.get(self.at).map(|(tok, pos)| (tok, *pos))
    }

    fn bump(&mut self) -> Option<(Tok, usize)> {
        let tok = self.tokens.get(self.at).cloned();
        self.at += 1;
        tok
    }

    fn or(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.and()?;
        while matches!(self.peek(), Some((Tok::Or, _))) {
            self.at += 1;
            left = Expr::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.unary()?;
        while matches!(self.peek(), Some((Tok::And, _))) {
            self.at += 1;
            left = Expr::And(Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        match self.bump() {
            Some((Tok::Not, _)) => Ok(Expr::Not(Box::new(self.unary()?))),
            Some((Tok::Open, pos)) => {
                let inner = self.or()?;
                match self.bump() {
                    Some((Tok::Close, _)) => Ok(inner),
                    Some((tok, at)) => Err(ExprError::new(at, format!("Expected “)” before {}", tok.describe()))),
                    None => Err(ExprError::new(pos, "This “(” is not closed")),
                }
            }
            Some((Tok::Word(name), pos)) => self.compare(&name, pos),
            Some((tok, pos)) => Err(ExprError::new(pos, format!("Expected a field before {}", tok.describe()))),
            None => Err(ExprError::new(self.end, "The condition ends too early")),
        }
    }

    fn compare(&mut self, name: &str, pos: usize) -> Result<Expr, ExprError> {
        let Some(field) = Field::parse(name) else {
            return Err(ExprError::new(pos, format!("Unknown field “{name}”; known fields: {}", FIELDS.join(", "))));
        };
        let op = match self.bump() {
            Some((Tok::Op(op), _)) => op,
            Some((tok, at)) => {
                return Err(ExprError::new(at, format!("Expected a comparison after {name}, found {}", tok.describe())));
            }
            None => return Err(ExprError::new(self.end, format!("Expected a comparison after {name}"))),
        };
        let (raw, at) = match self.bump() {
            Some((Tok::Str(text), at)) => (text, at),
            Some((Tok::Word(word), at)) => {
                return Err(ExprError::new(at, format!("Put the value in quotes: \"{word}\"")));
            }
            Some((tok, at)) => return Err(ExprError::new(at, format!("Expected a quoted value, found {}", tok.describe()))),
            None => return Err(ExprError::new(self.end, "Expected a quoted value")),
        };
        let value = value(field, op, &raw, at)?;
        Ok(Expr::Compare { field, op, value })
    }
}

fn value(field: Field, op: Op, raw: &str, at: usize) -> Result<Value, ExprError> {
    let lower = raw.to_lowercase();
    if matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge) && !field.ordered() {
        return Err(ExprError::new(at, format!("Only priority and status can use {}", op.word())));
    }
    match (field.values(), op) {
        (_, Op::Matches) => {
            let glob = GlobBuilder::new(&lower)
                .case_insensitive(true)
                .build()
                .map_err(|err| ExprError::new(at, format!("Not a valid pattern: {}", err.kind())))?;
            Ok(Value::Glob(Pattern { text: raw.to_string(), matcher: glob.compile_matcher() }))
        }
        (_, Op::Contains) => Ok(Value::Text(lower)),
        (Some(values), _) => {
            let wanted = lower.replace([' ', '-'], "_");
            match values.iter().position(|v| *v == wanted) {
                Some(rank) => Ok(Value::Rank(rank)),
                None => Err(ExprError::new(
                    at,
                    format!("Unknown {} “{raw}”; one of {}", field.name(), values.join(", ")),
                )),
            }
        }
        (None, _) => Ok(Value::Text(lower)),
    }
}
