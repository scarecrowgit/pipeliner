use super::*;
use crate::text::segment::*;

const FAMILY: &str = "👨‍👩‍👧"; // ZWJ sequence: one grapheme, 18 bytes
const FLAG: &str = "🇷🇺"; // regional indicator pair: one grapheme
const E_ACUTE: &str = "e\u{301}"; // combining mark: one grapheme, 3 bytes

fn buf(text: &str, cursor: usize) -> Buffer {
    let mut b = Buffer::new(text);
    b.set_cursor(cursor);
    b
}

/// Wraps every logical line after `width` graphemes, like a monospace soft wrap.
struct Wrapped {
    lines: Vec<(Range<usize>, bool)>, // (range, soft: continues on the next line)
    text: String,
}

impl Wrapped {
    fn new(text: &str, width: usize) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for line in text.split('\n') {
            let gs: Vec<usize> =
                unicode_segmentation::UnicodeSegmentation::grapheme_indices(line, true)
                    .map(|(i, _)| start + i)
                    .collect();
            let end = start + line.len();
            let mut chunk_starts: Vec<usize> = gs.iter().step_by(width).copied().collect();
            if chunk_starts.is_empty() {
                chunk_starts.push(start);
            }
            for (k, s) in chunk_starts.iter().enumerate() {
                let e = chunk_starts.get(k + 1).copied().unwrap_or(end);
                lines.push((*s..e, k + 1 < chunk_starts.len()));
            }
            start = end + 1;
        }
        Self {
            lines,
            text: text.to_string(),
        }
    }
}

impl VisualLines for Wrapped {
    fn line_count(&self) -> usize {
        self.lines.len()
    }
    fn line_for_offset(&self, offset: usize) -> usize {
        self.lines
            .iter()
            .rposition(|(r, _)| r.start <= offset)
            .unwrap_or(0)
    }
    fn x_for_offset(&self, offset: usize) -> f32 {
        let (r, _) = &self.lines[self.line_for_offset(offset)];
        unicode_segmentation::UnicodeSegmentation::graphemes(&self.text[r.start..offset], true)
            .count() as f32
    }
    fn offset_for_x(&self, line: usize, x: f32) -> usize {
        let (r, soft) = self.lines[line].clone();
        let at = unicode_segmentation::UnicodeSegmentation::grapheme_indices(
            &self.text[r.clone()],
            true,
        )
        .nth(x.round() as usize)
        .map_or(r.end, |(i, _)| r.start + i);
        if soft {
            at.min(prev_grapheme(&self.text, r.end))
        } else {
            at
        }
    }
}

// ── segment ─────────────────────────────────────────────────────────────

#[test]
fn graphemes_step_over_whole_clusters() {
    for g in ["a", "ж", FAMILY, FLAG, E_ACUTE, "😀"] {
        let t = format!("x{g}y");
        assert_eq!(next_grapheme(&t, 1), 1 + g.len(), "{g}");
        assert_eq!(prev_grapheme(&t, 1 + g.len()), 1, "{g}");
        for inside in 2..1 + g.len() {
            assert!(!is_grapheme_boundary(&t, inside) || g.len() == 1);
            assert_eq!(floor_grapheme(&t, inside), 1, "{g} at {inside}");
            assert_eq!(ceil_grapheme(&t, inside), 1 + g.len(), "{g} at {inside}");
        }
    }
    assert_eq!(prev_grapheme("", 0), 0);
    assert_eq!(next_grapheme("", 0), 0);
    assert_eq!(next_grapheme("ab", 99), 2);
    assert_eq!(prev_grapheme("ab", 99), 1);
}

#[test]
fn words_skip_spaces_and_punctuation() {
    let t = "hello, world! foo_bar";
    assert_eq!(next_word(t, 0), 5);
    assert_eq!(next_word(t, 5), 12);
    assert_eq!(next_word(t, 12), 21);
    assert_eq!(next_word(t, 21), 21);
    assert_eq!(prev_word(t, 21), 14);
    assert_eq!(prev_word(t, 14), 7);
    assert_eq!(prev_word(t, 9), 7);
    assert_eq!(prev_word(t, 7), 0);
    assert_eq!(prev_word(t, 0), 0);
    let c = "привет, мир";
    assert_eq!(next_word(c, 0), "привет".len());
    assert_eq!(prev_word(c, c.len()), "привет, ".len());
    let e = format!("hi {FAMILY} ok");
    assert_eq!(next_word(&e, 2), 3 + FAMILY.len());
    assert_eq!(prev_word("   ", 3), 0);
    assert_eq!(next_word("   ", 0), 3);
}

#[test]
fn word_and_line_at_an_offset() {
    let t = "foo bar_baz, qux\nnext";
    assert_eq!(word_at(t, 5), 4..11);
    assert_eq!(word_at(t, 0), 0..3);
    assert_eq!(word_at(t, 11), 11..12);
    assert_eq!(word_at(t, 16), 13..16, "on the newline: the word before");
    assert_eq!(word_at(t, t.len()), 17..21, "at the end: the last word");
    assert_eq!(word_at("a\n\nb", 2), 2..2, "an empty line selects nothing");
    assert_eq!(word_at("", 0), 0..0);
    assert_eq!(word_at("привет мир", 2), 0..12);
    assert_eq!(line_at(t, 5), 0..16);
    assert_eq!(line_at(t, 17), 17..21);
    assert_eq!(line_at(t, 16), 0..16);
    assert_eq!(line_at("", 0), 0..0);
}

#[test]
fn utf16_round_trips() {
    let t = format!("a😀ж{FAMILY}");
    assert_eq!(to_utf16(&t, 0), 0);
    assert_eq!(to_utf16(&t, 1), 1);
    assert_eq!(to_utf16(&t, 5), 3, "😀 is a surrogate pair");
    assert_eq!(to_utf16(&t, 7), 4, "ж is one unit");
    assert_eq!(to_utf16(&t, 3), 1, "inside a char counts up to its start");
    assert_eq!(from_utf16(&t, 3), 5);
    assert_eq!(
        from_utf16(&t, 2),
        1,
        "inside a surrogate pair: the char start"
    );
    assert_eq!(from_utf16(&t, 999), t.len());
    for (i, _) in t.char_indices() {
        assert_eq!(from_utf16(&t, to_utf16(&t, i)), i);
    }
    #[allow(clippy::reversed_empty_ranges)]
    let reversed = 4..1;
    assert_eq!(range_from_utf16(&t, &reversed), 1..7);
    assert_eq!(to_utf16("", 5), 0);
    assert_eq!(from_utf16("", 5), 0);
}

#[test]
fn newline_cleaning() {
    assert_eq!(normalize_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
    assert_eq!(flatten_newlines("a\r\nb\rc\nd"), "a b c d");
    assert_eq!(normalize_newlines("plain"), "plain");
}

// ── buffer ──────────────────────────────────────────────────────────────

#[test]
fn a_new_buffer_has_the_caret_at_the_end() {
    let b = Buffer::new("hello");
    assert_eq!((b.anchor(), b.head()), (5, 5));
    assert!(!b.has_selection());
    assert!(!b.can_undo());
}

#[test]
fn an_empty_buffer_ignores_every_operation() {
    let mut b = Buffer::new("");
    for m in [
        Motion::Left,
        Motion::Right,
        Motion::WordLeft,
        Motion::WordRight,
        Motion::LineStart,
        Motion::LineEnd,
        Motion::DocStart,
        Motion::DocEnd,
    ] {
        assert!(!b.delete(m));
        b.move_to(m, false);
        b.move_to(m, true);
        assert_eq!(b.selection(), 0..0);
    }
    b.move_vertically(true, false, &LogicalLines(""));
    b.move_vertically(false, true, &LogicalLines(""));
    b.select_all();
    b.select_word_at(3);
    b.select_line_at(3);
    assert_eq!(b.selection(), 0..0);
    assert!(!b.undo() && !b.redo());
    assert!(!b.insert(""));
    assert_eq!(b.token_before_cursor(&['@']), None);
    assert!(b.is_empty());
}

#[test]
fn typing_inserts_at_the_caret_and_replaces_the_selection() {
    let mut b = buf("ac", 1);
    assert!(b.commit_text(None, "b"));
    assert_eq!((b.text(), b.head()), ("abc", 2));
    b.select(0, 3);
    b.commit_text(None, "Ж");
    assert_eq!((b.text(), b.head()), ("Ж", 2));
    b.commit_text(None, FAMILY);
    assert_eq!(b.head(), 2 + FAMILY.len());
}

#[test]
fn backspace_and_delete_remove_whole_graphemes() {
    let text = format!("a{FAMILY}{E_ACUTE}{FLAG}ж");
    let mut b = buf(&text, text.len());
    b.delete(Motion::Left);
    assert_eq!(b.text(), format!("a{FAMILY}{E_ACUTE}{FLAG}"));
    b.delete(Motion::Left);
    assert_eq!(b.text(), format!("a{FAMILY}{E_ACUTE}"));
    b.delete(Motion::Left);
    assert_eq!(b.text(), format!("a{FAMILY}"));
    b.set_cursor(1);
    b.delete(Motion::Right);
    assert_eq!(b.text(), "a");
    assert!(!b.delete(Motion::Right), "delete at the end");
    b.set_cursor(0);
    assert!(!b.delete(Motion::Left), "backspace at the start");
    assert_eq!(b.text(), "a");
}

#[test]
fn the_caret_never_lands_inside_a_grapheme() {
    let text = format!("{FAMILY}{E_ACUTE}");
    let mut b = buf(&text, 5);
    assert_eq!(b.head(), 0);
    b.select(1, FAMILY.len() + 2);
    assert_eq!(b.selection(), 0..FAMILY.len());
    b.set_cursor(999);
    assert_eq!(b.head(), text.len());
    b.move_to(Motion::Left, false);
    assert_eq!(b.head(), FAMILY.len());
    b.move_to(Motion::Left, false);
    assert_eq!(b.head(), 0);
    b.move_to(Motion::Left, false);
    assert_eq!(b.head(), 0);
}

#[test]
fn left_and_right_collapse_a_selection_and_shift_extends() {
    let mut b = buf("hello", 1);
    b.move_to(Motion::Right, true);
    b.move_to(Motion::Right, true);
    assert_eq!((b.selection(), b.is_reversed()), (1..3, false));
    b.move_to(Motion::Left, false);
    assert_eq!(b.selection(), 1..1);
    b.select(4, 2);
    assert!(b.is_reversed());
    b.move_to(Motion::Right, false);
    assert_eq!(b.selection(), 4..4);
    b.move_to(Motion::Left, true);
    b.move_to(Motion::Left, true);
    b.move_to(Motion::Left, true);
    assert_eq!((b.selection(), b.head(), b.is_reversed()), (1..4, 1, true));
    b.move_to(Motion::DocEnd, true);
    assert_eq!((b.selection(), b.is_reversed()), (4..5, false));
}

#[test]
fn word_movement_and_deletion() {
    let text = "let x = привет_мир(42);";
    let mut b = buf(text, text.len());
    b.move_to(Motion::WordLeft, false);
    assert_eq!(&text[b.head()..], "42);");
    b.move_to(Motion::WordLeft, false);
    assert_eq!(&text[b.head()..], "привет_мир(42);");
    b.move_to(Motion::WordRight, true);
    assert_eq!(b.selected_text(), "привет_мир");
    b.move_to(Motion::DocEnd, false);
    b.delete(Motion::WordLeft);
    assert_eq!(b.text(), "let x = привет_мир(");
    b.set_cursor(0);
    b.delete(Motion::WordRight);
    assert_eq!(b.text(), " x = привет_мир(");
    b.move_to(Motion::DocEnd, false);
    b.delete(Motion::WordLeft);
    b.delete(Motion::WordLeft);
    b.delete(Motion::WordLeft);
    assert_eq!(b.text(), "");
    assert!(!b.delete(Motion::WordLeft));
}

#[test]
fn line_and_document_motions_use_logical_lines() {
    let text = "one\ntwo three\n\nlast";
    let mut b = buf(text, 6);
    b.move_to(Motion::LineStart, false);
    assert_eq!(b.head(), 4);
    b.move_to(Motion::LineEnd, true);
    assert_eq!(b.selected_text(), "two three");
    b.move_to(Motion::LineEnd, false);
    b.move_to(Motion::LineEnd, false);
    assert_eq!(b.head(), 13, "end stays at the end of the line");
    b.set_cursor(14);
    b.move_to(Motion::LineStart, false);
    assert_eq!(b.head(), 14, "an empty line starts and ends in one place");
    b.move_to(Motion::DocStart, true);
    assert_eq!(b.selection(), 0..14);
    b.move_to(Motion::DocEnd, false);
    assert_eq!(b.head(), text.len());
    b.set_cursor(8);
    b.delete(Motion::LineStart);
    assert_eq!(b.text(), "one\nthree\n\nlast");
    b.delete(Motion::LineEnd);
    assert_eq!(b.text(), "one\n\n\nlast");
}

#[test]
fn select_all_word_and_line() {
    let mut b = buf("alpha beta\ngamma", 0);
    b.select_all();
    assert_eq!(b.selection(), 0..16);
    b.select_word_at(7);
    assert_eq!(b.selected_text(), "beta");
    b.select_line_at(7);
    assert_eq!(b.selected_text(), "alpha beta");
    b.select_line_at(12);
    assert_eq!(b.selected_text(), "gamma");
    b.select_spanning(6..10, 0..5);
    assert_eq!((b.selection(), b.head()), (0..10, 0));
    b.select_spanning(6..10, 11..16);
    assert_eq!((b.selection(), b.head()), (6..16, 16));
}

#[test]
fn pasted_line_endings_are_normalised() {
    let mut b = Buffer::new("");
    b.insert("a\r\nb\rc");
    assert_eq!(b.text(), "a\nb\nc");
    assert_eq!(b.head(), 5);
    let mut s = Buffer::single_line("x\ny");
    assert_eq!(s.text(), "x y");
    s.insert("1\r\n2\n");
    assert_eq!(s.text(), "x y1 2 ");
    s.commit_text(None, "\n");
    assert!(!s.text().contains('\n'));
}

#[test]
fn a_protected_suffix_cannot_be_reached_or_edited() {
    let mut b = Buffer::single_line("notes.md");
    b.set_protected_suffix(3);
    assert_eq!(b.editable_end(), 5);
    assert_eq!(b.head(), 5);
    b.move_to(Motion::DocEnd, false);
    assert_eq!(b.head(), 5);
    b.move_to(Motion::Right, false);
    assert_eq!(b.head(), 5);
    assert!(!b.delete(Motion::Right));
    b.select_all();
    b.commit_text(None, "x");
    assert_eq!(b.text(), "x.md");
    b.replace_range(0..99, "plan");
    assert_eq!(b.text(), "plan.md");
    b.set_text("new");
    assert_eq!(b.editable_end(), 3);
}

#[test]
fn typing_is_undone_a_word_at_a_time() {
    let mut b = Buffer::new("");
    for c in "hello world".chars() {
        b.commit_text(None, &c.to_string());
    }
    assert!(b.undo());
    assert_eq!(b.text(), "hello");
    assert!(b.undo());
    assert_eq!(b.text(), "");
    assert!(!b.undo());
    assert!(b.redo());
    assert_eq!((b.text(), b.head()), ("hello", 5));
    assert!(b.redo());
    assert_eq!(b.text(), "hello world");
    assert!(!b.redo());
}

#[test]
fn a_caret_move_or_newline_starts_a_new_undo_group() {
    let mut b = Buffer::new("");
    b.commit_text(None, "a");
    b.commit_text(None, "b");
    b.move_to(Motion::Left, false);
    b.commit_text(None, "c");
    assert_eq!(b.text(), "acb");
    b.undo();
    assert_eq!(b.text(), "ab");
    b.move_to(Motion::DocEnd, false);
    b.commit_text(None, "\n");
    b.commit_text(None, "x");
    b.undo();
    assert_eq!(b.text(), "ab\n");
    b.undo();
    assert_eq!(b.text(), "ab");
}

#[test]
fn backspaces_group_and_undo_restores_the_selection() {
    let mut b = buf("hello", 5);
    for _ in 0..3 {
        b.delete(Motion::Left);
    }
    b.delete(Motion::Right);
    assert_eq!(b.text(), "he");
    b.undo();
    assert_eq!((b.text(), b.head()), ("hello", 5));
    b.select(1, 4);
    b.insert("ipp");
    assert_eq!(b.text(), "hippo");
    b.undo();
    assert_eq!((b.text(), b.selection(), b.head()), ("hello", 1..4, 4));
    b.redo();
    assert_eq!(b.text(), "hippo");
    b.undo();
    b.commit_text(None, "!");
    assert!(!b.can_redo(), "a new edit drops the redo stack");
}

#[test]
fn typing_over_a_selection_is_its_own_group() {
    let mut b = Buffer::new("abc");
    b.commit_text(None, "d");
    b.select_all();
    b.commit_text(None, "x");
    b.commit_text(None, "y");
    assert_eq!(b.text(), "xy");
    b.undo();
    assert_eq!(b.text(), "abcd");
}

#[test]
fn ime_composition_marks_text_and_commits_as_one_step() {
    let mut b = Buffer::new("a");
    b.mark_text(None, "k", None);
    assert_eq!(b.marked(), Some(1..2));
    b.mark_text(None, "か", None);
    assert_eq!((b.text(), b.marked(), b.head()), ("aか", Some(1..4), 4));
    b.mark_text(None, "かn", Some(0..3));
    assert_eq!(b.selection(), 1..4, "selection inside the new text");
    b.commit_text(None, "かん");
    assert_eq!((b.text(), b.marked(), b.head()), ("aかん", None, 7));
    b.undo();
    assert_eq!(b.text(), "a");
    // Cancelled composition leaves nothing behind.
    b.mark_text(None, "´", None);
    b.mark_text(None, "", None);
    assert_eq!((b.text(), b.marked()), ("a", None));
    // A dead key followed by a letter.
    b.mark_text(None, "´", None);
    b.commit_text(None, "é");
    assert_eq!(b.text(), "aé");
}

#[test]
fn explicit_ranges_from_the_platform_replace_that_range() {
    let mut b = Buffer::new("hello");
    b.commit_text(Some(1..3), "EE");
    assert_eq!((b.text(), b.head()), ("hEElo", 3));
    b.mark_text(Some(0..1), "H", None);
    assert_eq!(b.marked(), Some(0..1));
    b.unmark();
    assert_eq!(b.marked(), None);
    // Out-of-range and reversed ranges are clamped, not a panic.
    b.commit_text(Some(4..99), "");
    assert_eq!(b.text(), "HEEl");
    #[allow(clippy::reversed_empty_ranges)]
    b.commit_text(Some(3..1), "-");
    assert_eq!(b.text(), "H-l");
}

#[test]
fn utf16_offsets_through_the_buffer() {
    let b = Buffer::new("я😀z");
    assert_eq!(b.to_utf16(b.len()), 4);
    assert_eq!(b.from_utf16(3), 6);
    assert_eq!(b.range_to_utf16(&(0..6)), 0..3);
    assert_eq!(b.range_from_utf16(&(1..3)), 2..6);
}

#[test]
fn vertical_movement_keeps_the_goal_column() {
    let text = "long line one\nab\nanother line";
    let lines = LogicalLines(text);
    let mut b = buf(text, 10);
    b.move_vertically(true, false, &lines);
    assert_eq!(b.head(), 16, "clamped to the short line's end");
    b.move_vertically(true, false, &lines);
    assert_eq!(b.head(), 17 + 10, "goal column 10 restored");
    b.move_vertically(true, false, &lines);
    assert_eq!(
        b.head(),
        text.len(),
        "down on the last line goes to the end"
    );
    b.move_vertically(false, false, &lines);
    assert_eq!(b.head(), 16);
    b.move_vertically(false, false, &lines);
    b.move_vertically(false, false, &lines);
    assert_eq!(b.head(), 0, "up on the first line goes to the start");
    b.move_to(Motion::Right, false);
    assert_eq!(b.goal_x(), None, "any other motion forgets the goal");
}

#[test]
fn vertical_movement_extends_and_collapses() {
    let text = "abc\ndef\nghi";
    let lines = LogicalLines(text);
    let mut b = buf(text, 1);
    b.move_vertically(true, true, &lines);
    assert_eq!(b.selection(), 1..5);
    b.move_vertically(true, true, &lines);
    assert_eq!(b.selection(), 1..9);
    b.select(5, 9);
    b.move_vertically(false, false, &lines);
    assert_eq!(b.selection(), 1..1, "up moves from the selection start");
    b.select(1, 5);
    b.move_vertically(true, false, &lines);
    assert_eq!(b.selection(), 9..9, "down moves from the selection end");
}

#[test]
fn vertical_movement_follows_soft_wraps() {
    // Width 4: "abcd|efgh|ij" then "xy".
    let text = "abcdefghij\nxy";
    let lines = Wrapped::new(text, 4);
    assert_eq!(lines.line_count(), 4);
    assert_eq!(
        lines.line_for_offset(4),
        1,
        "a wrap offset belongs to the next line"
    );
    let mut b = buf(text, 2);
    b.move_vertically(true, false, &lines);
    assert_eq!(b.head(), 6);
    b.move_vertically(true, false, &lines);
    assert_eq!(b.head(), 10, "the short last visual line clamps to its end");
    b.move_vertically(true, false, &lines);
    assert_eq!(b.head(), 13, "then the next logical line, column 2");
    b.move_vertically(false, false, &lines);
    b.move_vertically(false, false, &lines);
    assert_eq!(b.head(), 6);
    b.set_cursor(3);
    b.move_vertically(true, false, &lines);
    b.set_cursor(7);
    b.move_vertically(false, false, &lines);
    assert_eq!(b.head(), 3, "from column 3 on line 1 to column 3 on line 0");
}

#[test]
fn vertical_movement_over_emoji_lands_on_graphemes() {
    let text = format!("{FAMILY}{FAMILY}{FAMILY}\nab");
    let lines = Wrapped::new(&text, 2);
    let mut b = buf(&text, 0);
    b.move_to(Motion::Right, false);
    b.move_vertically(true, false, &lines);
    assert_eq!(
        b.head(),
        3 * FAMILY.len(),
        "the one-emoji wrapped line clamps to its end"
    );
    b.move_vertically(true, false, &lines);
    assert_eq!(b.head(), 3 * FAMILY.len() + 2, "column 1 of \"ab\"");
    b.move_vertically(false, false, &lines);
    b.move_vertically(false, false, &lines);
    assert_eq!(b.head(), FAMILY.len());
    assert!(is_grapheme_boundary(b.text(), b.head()));
}

#[test]
fn tokens_are_found_only_at_a_word_start() {
    let mut b = Buffer::new("@re");
    assert_eq!(
        b.token_before_cursor(&['@', '/']),
        Some(Token {
            trigger: '@',
            start: 0,
            text: "re".into()
        })
    );
    b.set_text("ask @rev");
    let t = b.token_before_cursor(&['@']);
    assert_eq!(
        t,
        Some(Token {
            trigger: '@',
            start: 4,
            text: "rev".into()
        })
    );
    b.set_text("mail me@host");
    assert_eq!(b.token_before_cursor(&['@']), None);
    b.set_text("/");
    assert_eq!(
        b.token_before_cursor(&['/']).map(|t| t.text),
        Some(String::new())
    );
    b.set_text("x\n/mod");
    assert_eq!(b.token_before_cursor(&['/']).map(|t| t.start), Some(2));
    b.set_text("@done ");
    assert_eq!(
        b.token_before_cursor(&['@']),
        None,
        "a space ends the token"
    );
    b.set_text("привет @мир");
    assert_eq!(
        b.token_before_cursor(&['@']).map(|t| t.text),
        Some("мир".into())
    );
    b.select_all();
    assert_eq!(b.token_before_cursor(&['@']), None);
}

#[test]
fn tokens_are_replaced_in_place() {
    let mut b = Buffer::new("ask @re now");
    b.set_cursor(7);
    let t = b.token_before_cursor(&['@']).expect("token");
    assert!(b.replace_token(&t, "@reviewer "));
    assert_eq!((b.text(), b.head()), ("ask @reviewer  now", 14));
    b.undo();
    assert_eq!(b.text(), "ask @re now");
    // The text moved on: the current token with the same trigger is used.
    b.set_cursor(7);
    b.commit_text(None, "v");
    assert!(b.replace_token(&t, "@reviewer"));
    assert_eq!(b.text(), "ask @reviewer now");
    b.set_cursor(0);
    assert!(!b.replace_token(&t, "x"));
}
