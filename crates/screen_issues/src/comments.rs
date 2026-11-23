//! Issue comments: `@mention` and `` `code` `` parsing and the autocomplete matching.
//!
//! Plain data in / data out over a list of mentionable names (the configured pipelines),
//! so it is unit-tested without gpui.

use std::ops::Range;

/// A piece of comment text. Ranges are byte ranges into the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(Range<usize>),
    /// `@Name` (range covers the `@` and the name as typed); index into the names.
    Mention(Range<usize>, usize),
    /// Text between backticks (range excludes the backticks).
    Code(Range<usize>),
}

/// The name following the `@` at the start of `rest`: the longest name wins and must end
/// at the end of the text or before a non-alphanumeric char. Returns (index, byte length).
fn mention_at<S: AsRef<str>>(rest: &str, names: &[S]) -> Option<(usize, usize)> {
    names
        .iter()
        .map(AsRef::as_ref)
        .enumerate()
        .filter(|(_, n)| {
            !n.is_empty()
                && rest.len() >= n.len()
                && rest.is_char_boundary(n.len())
                && rest[..n.len()].eq_ignore_ascii_case(n)
                && !rest[n.len()..].chars().next().is_some_and(char::is_alphanumeric)
        })
        .max_by_key(|(_, n)| n.len())
        .map(|(ix, n)| (ix, n.len()))
}

pub fn spans<S: AsRef<str>>(text: &str, names: &[S]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain_start = 0;
    let mut prev: Option<char> = None;
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let special = match c {
            '`' => text[i + 1..]
                .find('`')
                .filter(|&n| n > 0)
                .map(|n| (Span::Code(i + 1..i + 1 + n), i + n + 2)),
            '@' if prev.is_none_or(char::is_whitespace) => mention_at(&text[i + 1..], names)
                .map(|(ix, len)| (Span::Mention(i..i + 1 + len, ix), i + 1 + len)),
            _ => None,
        };
        if let Some((span, end)) = special {
            if plain_start < i {
                out.push(Span::Text(plain_start..i));
            }
            out.push(span);
            plain_start = end;
            prev = text[..end].chars().next_back();
            i = end;
        } else {
            prev = Some(c);
            i += c.len_utf8();
        }
    }
    if plain_start < text.len() {
        out.push(Span::Text(plain_start..text.len()));
    }
    out
}

/// Mentioned names (as in `names`), unique, in order of first appearance.
pub fn mentions<S: AsRef<str>>(text: &str, names: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in spans(text, names) {
        if let Span::Mention(_, ix) = span {
            let n = names[ix].as_ref();
            if !out.iter().any(|o| o == n) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// The `@token` being typed at the end of `text`: byte offset of the `@` and the token.
pub fn query(text: &str) -> Option<(usize, &str)> {
    let start = text
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + text[i..].chars().next().map_or(1, char::len_utf8));
    text[start..].strip_prefix('@').map(|token| (start, token))
}

/// Names where some word starts with `query`, with the matched byte range inside the
/// name. An empty query matches everything.
pub fn matches<S: AsRef<str>>(query: &str, names: &[S]) -> Vec<(usize, Range<usize>)> {
    let n = query.len();
    names
        .iter()
        .map(AsRef::as_ref)
        .enumerate()
        .filter_map(|(ix, name)| {
            let mut offset = 0;
            for word in name.split(' ') {
                if word.len() >= n && word.is_char_boundary(n) && word[..n].eq_ignore_ascii_case(query) {
                    return Some((ix, offset..offset + n));
                }
                offset += word.len() + 1;
            }
            None
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 3] = ["Mockup build", "Mockup", "Bugfix"];

    #[test]
    fn spans_mentions_and_code() {
        let t = "hi @mockup build, see `x.rs` @Bugfix";
        let s = spans(t, &NAMES);
        assert_eq!(
            s,
            vec![
                Span::Text(0..3),
                Span::Mention(3..16, 0),
                Span::Text(16..22),
                Span::Code(23..27),
                Span::Text(28..29),
                Span::Mention(29..36, 2),
            ]
        );
        assert_eq!(mentions(t, &NAMES), vec!["Mockup build", "Bugfix"]);
    }

    #[test]
    fn mention_needs_word_boundaries() {
        assert!(mentions("mail@Bugfix", &NAMES).is_empty());
        assert!(mentions("@Bugfixes", &NAMES).is_empty());
        assert_eq!(mentions("@Mockup, @mockup", &NAMES), vec!["Mockup"]);
        assert!(mentions("@Мок", &NAMES).is_empty());
        assert_eq!(spans("`` @", &NAMES), vec![Span::Text(0..4)]);
    }

    #[test]
    fn typing_query() {
        assert_eq!(query("hello @mo"), Some((6, "mo")));
        assert_eq!(query("@"), Some((0, "")));
        assert_eq!(query("hello @mo "), None);
        assert_eq!(query("a\u{3000}@b"), Some((4, "b")));
    }

    #[test]
    fn autocomplete() {
        assert_eq!(matches("bu", &NAMES), vec![(0, 7..9), (2, 0..2)]);
        assert_eq!(matches("", &NAMES).len(), 3);
        assert!(matches("zz", &NAMES).is_empty());
    }
}
