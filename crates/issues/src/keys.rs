//! Issue keys: `HAR-12`, their natural order and the prefix a project gets.

use std::cmp::Ordering;

#[cfg(test)]
mod tests;

/// Splits `HAR-12` into `("HAR", 12)`. The prefix is ASCII letters (returned upper-cased), the
/// number plain digits that fit a `u32`. Anything else is not a key.
pub fn parse(key: &str) -> Option<(String, u32)> {
    let (prefix, number) = key.trim().split_once('-')?;
    if prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((prefix.to_ascii_uppercase(), number.parse().ok()?))
}

/// `("HAR", 12)` as `HAR-12`.
pub fn format(prefix: &str, number: u32) -> String {
    format!("{prefix}-{number}")
}

/// Natural order: by prefix, then by number, so `HAR-9` comes before `HAR-10`. Text that is
/// not a key sorts after every key, in plain string order.
pub fn compare(a: &str, b: &str) -> Ordering {
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

/// The number the next issue of `prefix` gets, given the keys that exist.
pub fn next_number<'a>(prefix: &str, existing: impl IntoIterator<Item = &'a str>) -> u32 {
    let prefix = prefix.to_ascii_uppercase();
    existing
        .into_iter()
        .filter_map(parse)
        .filter(|(p, _)| *p == prefix)
        .map(|(_, n)| n)
        .max()
        .map_or(1, |n| n.saturating_add(1))
}

/// The key prefix of a new project: 2 to 4 upper-case ASCII letters, never empty.
///
/// One word gives its first three letters (`Payments` is `PAY`); several words give their
/// initials, up to four (`Checkout web app` is `CWA`). A name with no ASCII letters gives
/// `PRJ`; a single letter is padded with `X`.
pub fn prefix_for(project_name: &str) -> String {
    let words: Vec<&str> =
        project_name.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| !w.is_empty()).collect();
    let mut prefix: String = match words.as_slice() {
        [] => return "PRJ".to_string(),
        [word] => word.chars().take(3).collect(),
        many => many.iter().filter_map(|w| w.chars().next()).take(4).collect(),
    };
    prefix.make_ascii_uppercase();
    while prefix.len() < 2 {
        prefix.push('X');
    }
    prefix
}
