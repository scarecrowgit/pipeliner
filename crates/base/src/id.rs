//! Unique ids for rows and files, and slugs for branch and directory names.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
/// Width of the timestamp part: 36^9 ms reaches past the year 5000, so ids keep one length.
const TIME_WIDTH: usize = 9;
/// Width of the per-millisecond sequence part.
const SEQ_WIDTH: usize = 2;
const SEQ_LIMIT: u32 = 36 * 36;
/// Width of the random part (about 41 bits).
const RANDOM_WIDTH: usize = 8;

/// Last timestamp handed out and how many ids it has produced.
static LAST: Mutex<(i64, u32)> = Mutex::new((0, 0));
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A new unique id, e.g. `0mhv3k2p1-00x7f3k2a9q`.
///
/// Ids sort by creation time as plain strings: a zero-padded base36 millisecond timestamp, a
/// `-`, then a per-millisecond sequence and a random suffix. Within one process every id is
/// greater than the one before it; across processes ids are ordered to the millisecond and
/// kept apart by the random suffix. Only lowercase letters, digits and one `-`, so an id is
/// safe as a file name.
pub fn new_id() -> String {
    let now = crate::time::now_ms().max(0);
    let (ms, seq) = {
        let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if now > last.0 {
            *last = (now, 0);
        } else {
            // Same millisecond, or the wall clock stepped back: stay ahead of the last id.
            last.1 += 1;
            if last.1 >= SEQ_LIMIT {
                *last = (last.0 + 1, 0);
            }
        }
        *last
    };
    let mut id = String::with_capacity(TIME_WIDTH + 1 + SEQ_WIDTH + RANDOM_WIDTH);
    push_base36(&mut id, ms as u64, TIME_WIDTH);
    id.push('-');
    push_base36(&mut id, u64::from(seq), SEQ_WIDTH);
    push_base36(&mut id, random(), RANDOM_WIDTH);
    id
}

/// A value that differs between calls and between processes. `RandomState` is seeded from
/// the OS once per thread; the pid, a counter and the clock are mixed in on top.
fn random() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    hasher.write_u32(nanos);
    hasher.finish()
}

/// Appends the lowest `width` base36 digits of `value`, zero-padded.
fn push_base36(out: &mut String, mut value: u64, width: usize) {
    let mut digits = [b'0'; 16];
    let width = width.min(digits.len());
    for slot in digits[..width].iter_mut().rev() {
        *slot = BASE36[(value % 36) as usize];
        value /= 36;
    }
    out.extend(digits[..width].iter().map(|&b| char::from(b)));
}

/// A name safe for branches, directories and URLs: lowercase ASCII letters, digits and single
/// `-` separators, at most `max_len` characters, cut at a word boundary when one is near.
///
/// Cyrillic and common Latin diacritics are transliterated (`"Починить вход"` becomes
/// `pochinit-vkhod`); anything else separates words. Never empty: text with nothing usable
/// gives `item`.
pub fn slug(text: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        let mut buf = [0u8; 4];
        let piece: &str = if c.is_ascii_alphanumeric() {
            c.encode_utf8(&mut buf)
        } else if let Some(latin) = transliterate(c) {
            latin
        } else {
            pending_sep = true;
            continue;
        };
        if piece.is_empty() {
            continue;
        }
        if pending_sep && !out.is_empty() {
            out.push('-');
        }
        pending_sep = false;
        out.push_str(piece);
    }
    if out.len() > max_len {
        // All ASCII here, so byte positions are character positions.
        let cut_at_word = out.as_bytes()[max_len] == b'-';
        out.truncate(max_len);
        if !cut_at_word && let Some(i) = out.rfind('-').filter(|&i| i >= max_len / 2) {
            out.truncate(i);
        }
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() { "item".to_string() } else { out }
}

/// Latin spelling of a lowercase character; `None` when it has none and separates words.
fn transliterate(c: char) -> Option<&'static str> {
    Some(match c {
        // Russian, Ukrainian and Belarusian Cyrillic.
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' | 'ґ' => "g",
        'д' => "d",
        'е' | 'ё' | 'э' => "e",
        'є' => "ye",
        'ж' => "zh",
        'з' => "z",
        'и' | 'і' => "i",
        'ї' => "yi",
        'й' | 'ы' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' | 'ў' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' | 'ь' => "",
        'ю' => "yu",
        'я' => "ya",
        // Common Latin diacritics.
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'ł' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' => "r",
        'ś' | 'š' => "s",
        'ß' => "ss",
        'ť' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        // Apostrophes join words ("don't" gives "dont").
        '\'' | '’' => "",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn is_slug(s: &str) -> bool {
        !s.is_empty()
            && !s.starts_with('-')
            && !s.ends_with('-')
            && !s.contains("--")
            && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    #[test]
    fn ids_have_one_shape() {
        let id = new_id();
        assert_eq!(id.len(), TIME_WIDTH + 1 + SEQ_WIDTH + RANDOM_WIDTH);
        assert_eq!(id.as_bytes()[TIME_WIDTH], b'-');
        assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        assert_eq!(id.matches('-').count(), 1);
    }

    #[test]
    fn ids_are_unique_and_increasing_within_a_process() {
        let ids: Vec<String> = (0..5_000).map(|_| new_id()).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn ids_are_unique_across_threads() {
        let threads: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..500).map(|_| new_id()).collect::<Vec<_>>()))
            .collect();
        let mut seen = HashSet::new();
        for t in threads {
            for id in t.join().unwrap() {
                assert!(seen.insert(id));
            }
        }
        assert_eq!(seen.len(), 4_000);
    }

    #[test]
    fn id_timestamp_decodes_to_now() {
        let before = crate::time::now_ms();
        let id = new_id();
        let ms = i64::from_str_radix(&id[..TIME_WIDTH], 36).unwrap();
        // Other tests may have pushed the sequence a little ahead of the clock.
        assert!(ms >= before && ms <= crate::time::now_ms() + 1_000, "{ms} vs {before}");
    }

    #[test]
    fn base36_is_zero_padded_and_ordered() {
        let enc = |v: u64, w: usize| {
            let mut s = String::new();
            push_base36(&mut s, v, w);
            s
        };
        assert_eq!(enc(0, 4), "0000");
        assert_eq!(enc(35, 2), "0z");
        assert_eq!(enc(36, 2), "10");
        assert_eq!(enc(36 * 36, 2), "00");
        assert_eq!(enc(u64::MAX, 40).len(), 16);
        assert!(enc(1_700_000_000_000, 9) < enc(1_800_000_000_000, 9));
    }

    #[test]
    fn slugs_from_plain_text() {
        assert_eq!(slug("Fix the login page", 40), "fix-the-login-page");
        assert_eq!(slug("  --Hello,   World!!  ", 40), "hello-world");
        assert_eq!(slug("HAR-4: comments in issue", 40), "har-4-comments-in-issue");
        assert_eq!(slug("don't stop", 40), "dont-stop");
        assert_eq!(slug("a_b/c.d", 40), "a-b-c-d");
    }

    #[test]
    fn slugs_transliterate() {
        assert_eq!(slug("Починить вход", 40), "pochinit-vkhod");
        assert_eq!(slug("Ещё щи, объём", 40), "eshche-shchi-obem");
        assert_eq!(slug("Їжак Ґанок", 40), "yizhak-ganok");
        assert_eq!(slug("Crème brûlée straße", 40), "creme-brulee-strasse");
    }

    #[test]
    fn slugs_are_never_empty() {
        assert_eq!(slug("", 40), "item");
        assert_eq!(slug("   ", 40), "item");
        assert_eq!(slug("日本語", 40), "item");
        assert_eq!(slug("!!!", 40), "item");
        assert_eq!(slug("ьъ'", 40), "item");
        assert_eq!(slug("anything", 0), "item");
    }

    #[test]
    fn slugs_respect_the_length_limit() {
        assert_eq!(slug("abcdefghij", 4), "abcd");
        assert_eq!(slug("abcd efgh", 4), "abcd");
        assert_eq!(slug("abcd efgh", 5), "abcd");
        // Cut back to a word boundary when it is in the second half.
        assert_eq!(slug("make the sidebar resizable", 20), "make-the-sidebar");
        // No boundary near the end: a hard cut.
        assert_eq!(slug("a extraordinarily", 10), "a-extraord");
        let long = "слово ".repeat(50);
        for max in 1..60 {
            let s = slug(&long, max);
            assert!(s.len() <= max, "{s} > {max}");
            assert!(is_slug(&s), "{s}");
        }
    }

    #[test]
    fn slugs_are_always_well_formed() {
        for text in ["-a-", "a - b", "---", "A--B", "x\u{0}y", "tab\there", "ünï cödé", "1 2 3"] {
            for max in [1, 2, 3, 5, 8, 40] {
                assert!(is_slug(&slug(text, max)), "{text:?} at {max}");
            }
        }
    }
}
