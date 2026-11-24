//! The number style of the mockups: `48.2k` tokens, `$0.21` cost, `4:12` durations, `412 KB`.
//!
//! All functions are pure and never panic, whatever the input (NaN, negative or huge values
//! included). Screens call these instead of formatting numbers themselves, so a token count
//! reads the same in the sidebar, the board and the agent list.

#[cfg(test)]
mod tests;

/// Scaling steps for token counts: plain below 1000, then `k`, `M`, `B`.
const TOKEN_UNITS: [(u64, &str); 3] = [(1_000, "k"), (1_000_000, "M"), (1_000_000_000, "B")];
/// Scaling steps for sizes (decimal, as file managers show them).
const BYTE_UNITS: [(u64, &str); 4] =
    [(1_000, " KB"), (1_000_000, " MB"), (1_000_000_000, " GB"), (1_000_000_000_000, " TB")];

/// A token count, short: `950`, `3.2k`, `48.2k`, `200k`, `1.3M`.
///
/// One decimal at most; a trailing `.0` is dropped. A value that rounds up to the next unit
/// moves to it (`999_950` is `1M`, not `1000k`).
pub fn tokens(n: u64) -> String {
    if n < TOKEN_UNITS[0].0 {
        return n.to_string();
    }
    compact(n, &TOKEN_UNITS, u64::MAX)
}

/// A money amount for rows and cards: `$0.00`, `<$0.01`, `$0.21`, `$612.40`, `$1,234.50`.
///
/// Zero, negative, NaN and infinite amounts show as `$0.00`; anything positive that would
/// round to zero shows as `<$0.01`.
pub fn cost(usd: f64) -> String {
    if !usd.is_finite() || usd <= 0.0 {
        return "$0.00".to_string();
    }
    if usd < 0.005 {
        return "<$0.01".to_string();
    }
    let cents = to_cents(usd);
    format!("${}.{:02}", count(cents / 100), cents % 100)
}

/// A money amount for chart axes: whole dollars lose the cents (`$10`), others keep them
/// (`$3.50`). Zero, negative and non-finite amounts give `$0`.
pub fn cost_short(usd: f64) -> String {
    if !usd.is_finite() || usd <= 0.0 {
        return "$0".to_string();
    }
    let cents = to_cents(usd);
    if cents.is_multiple_of(100) {
        format!("${}", count(cents / 100))
    } else {
        format!("${}.{:02}", count(cents / 100), cents % 100)
    }
}

/// An elapsed time as a clock: `42s`, `1:42`, `4:12`, `1h 1m`, `1d 1h`.
pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = split(secs);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}:{s:02}")
    } else {
        format!("{s}s")
    }
}

/// An elapsed time as a stopwatch, always with minutes: `0:00`, `0:30`, `1:42`, `59:59`,
/// and past an hour `1:00:00`, `25:03:07` (hours keep counting, no days). For the marks of
/// a time axis, where every label has the same shape.
pub fn duration_clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3_600, secs % 3_600 / 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// An elapsed time in words: `6s`, `4m 12s`, `2h`, `1h 1m`, `1d 1h`.
///
/// Shows the two largest units and leaves the second out when it is zero.
pub fn duration_words(secs: u64) -> String {
    let (d, h, m, s) = split(secs);
    let (big, big_unit, small, small_unit) = if d > 0 {
        (d, "d", h, "h")
    } else if h > 0 {
        (h, "h", m, "m")
    } else if m > 0 {
        (m, "m", s, "s")
    } else {
        (s, "s", 0, "")
    };
    if small == 0 { format!("{big}{big_unit}") } else { format!("{big}{big_unit} {small}{small_unit}") }
}

/// A whole number with thousands separators: `2418` becomes `2,418`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A size in bytes, decimal units: `12 B`, `412 KB`, `1.1 MB`.
///
/// One decimal below 10 units, whole numbers above.
pub fn bytes(n: u64) -> String {
    if n < BYTE_UNITS[0].0 {
        return format!("{n} B");
    }
    compact(n, &BYTE_UNITS, 100)
}

/// `part` as a share of `whole`, rounded and clamped to `0..=100`.
///
/// Zero when `whole` is zero, negative or NaN, or when `part` is NaN, so a progress bar never
/// gets a nonsense width.
pub fn percent(part: f64, whole: f64) -> u32 {
    // `whole > 0.0` is false for NaN too.
    let whole_ok = whole > 0.0;
    if !whole_ok || part.is_nan() {
        return 0;
    }
    let share = (part / whole * 100.0).round();
    if share.is_nan() { 0 } else { share.clamp(0.0, 100.0) as u32 }
}

/// A count with its noun: `plural(1, "file", "files")` is `1 file`, `3` gives `3 files`.
///
/// The number gets thousands separators, as [`count`] does.
pub fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// The first line of `text` that has anything on it, trimmed. Empty when there is none.
pub fn first_line(text: &str) -> &str {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("")
}

/// `text` cut to at most `max` characters, the last one being `…` when something was cut.
///
/// Counts `char`s, never splits one, and returns `text` untouched when it already fits.
pub fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.char_indices();
    match chars.nth(max) {
        None => text.to_string(),
        Some(_) if max == 0 => String::new(),
        Some(_) => {
            let keep = text.char_indices().nth(max - 1).map_or(text.len(), |(i, _)| i);
            format!("{}…", &text[..keep])
        }
    }
}

/// Days, hours, minutes and seconds of a number of seconds.
fn split(secs: u64) -> (u64, u64, u64, u64) {
    (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60, secs % 60)
}

/// Dollars to whole cents, rounded; saturates for huge values.
fn to_cents(usd: f64) -> u64 {
    (usd * 100.0).round() as u64
}

/// Scales `n` to the largest of `units` (ascending, all `n >= units[0].0`) with one decimal.
///
/// From `whole_from` tenths on (10 units = 100 tenths) the decimal is dropped. A value that
/// rounds up to 1000 units moves to the next unit.
fn compact(n: u64, units: &[(u64, &str)], whole_from: u64) -> String {
    let mut i = units.iter().rposition(|&(div, _)| n >= div).unwrap_or(0);
    loop {
        let (div, suffix) = units[i];
        let mut tenths = n.saturating_add(div / 20) / (div / 10);
        if tenths >= whole_from {
            tenths = n.saturating_add(div / 2) / div * 10;
        }
        if tenths >= 10_000 && i + 1 < units.len() {
            i += 1;
            continue;
        }
        return if tenths.is_multiple_of(10) {
            format!("{}{suffix}", tenths / 10)
        } else {
            format!("{}.{}{suffix}", tenths / 10, tenths % 10)
        };
    }
}
