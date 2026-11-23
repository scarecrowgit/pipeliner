use super::SAMPLE;
use crate::markdown::parse::parse;
use crate::markdown::rich::{
    RichText, UnitKind, first_paragraph, plain_text, split_mentions, units,
};
use crate::markdown::selection::{Selection, TextPoint, word_range};

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| s.to_string()).collect()
}

#[test]
fn mentions_need_word_boundaries() {
    let m = names(&["Fixer", "Fix", "Codex"]);
    let t = "hi @Fixer, mail a@Codex or @Fixers @Codex";
    let got: Vec<(&str, bool)> = split_mentions(t, &m)
        .into_iter()
        .map(|(r, f)| (&t[r], f))
        .collect();
    assert_eq!(
        got,
        [
            ("hi ", false),
            ("@Fixer", true),
            (", mail a@Codex or @Fixers ", false),
            ("@Codex", true)
        ]
    );
    assert_eq!(split_mentions("", &m), [(0..0, false)]);
}

#[test]
fn rich_text_spans_cover_the_text() {
    let doc = parse("Ask **@Fixer** about `code` and [docs](u) now, @Codex.");
    let crate::markdown::Block::Paragraph(runs) = &doc.blocks[0] else {
        panic!()
    };
    let rt = RichText::from_inlines(runs, &names(&["Fixer", "Codex"]));
    assert_eq!(rt.text, "Ask @Fixer about code and docs now, @Codex.");
    let mut at = 0;
    for s in &rt.spans {
        assert_eq!(s.range.start, at);
        at = s.range.end;
    }
    assert_eq!(at, rt.text.len());
    let style_of = |t: &str| {
        let start = rt.text.find(t).unwrap_or(usize::MAX);
        rt.spans
            .iter()
            .find(|s| s.range.start == start)
            .map(|s| s.style)
    };
    let fixer = style_of("@Fixer").unwrap_or_default();
    assert!(fixer.mention && fixer.bold);
    assert!(style_of("code").unwrap_or_default().code);
    assert!(style_of("@Codex").unwrap_or_default().mention);
    assert_eq!(rt.links.len(), 1);
    assert_eq!(&rt.text[rt.links[0].0.clone()], "docs");
    assert_eq!(rt.link_at(rt.text.find("docs").unwrap_or(0)), Some(0));
    assert_eq!(rt.link_at(0), None);
}

#[test]
fn code_text_expands_tabs() {
    assert_eq!(RichText::plain("\tx").text, "    x");
}

#[test]
fn units_follow_reading_order() {
    let doc = parse(SAMPLE);
    let us = units(&doc, &[]);
    let kinds: Vec<&UnitKind> = us.iter().map(|u| &u.kind).collect();
    assert_eq!(kinds[0], &UnitKind::Heading(1));
    assert!(
        us.iter()
            .any(|u| matches!(&u.kind, UnitKind::Code { lang, .. } if lang == "ts"))
    );
    assert_eq!(
        us.iter().filter(|u| u.kind == UnitKind::HeadCell).count(),
        3
    );
    assert_eq!(us.iter().filter(|u| u.kind == UnitKind::Cell).count(), 6);
    assert!(us.iter().all(|u| !u.joiner.is_empty()));
}

#[test]
fn select_all_copies_sensible_plain_text() {
    let doc = parse(
        "# Title\n\nPara one.\n\n- a\n- b\n  - c\n\n1. x\n\n| h1 | h2 |\n|--|--|\n| 1 | 2 |\n\n```\ncode\n```",
    );
    let us = units(&doc, &[]);
    let mut sel = Selection::default();
    sel.select_all(&us);
    let text = sel.text(&us).unwrap_or_default();
    assert_eq!(
        text,
        "Title\n\nPara one.\n\n- a\n- b\n  - c\n\n1. x\n\nh1\th2\n1\t2\n\ncode"
    );
}

#[test]
fn partial_selection_across_blocks_either_direction() {
    let doc = parse("hello world\n\nsecond\n\nthird one");
    let us = units(&doc, &[]);
    let mut sel = Selection {
        anchor: Some(TextPoint { unit: 2, offset: 5 }),
        head: Some(TextPoint { unit: 0, offset: 6 }),
    };
    assert_eq!(sel.text(&us).as_deref(), Some("world\n\nsecond\n\nthird"));
    assert_eq!(sel.range_in(1, 6), Some(0..6));
    assert_eq!(sel.range_in(0, 11), Some(6..11));
    assert_eq!(sel.range_in(2, 9), Some(0..5));
    sel.collapse(TextPoint { unit: 1, offset: 2 });
    assert!(sel.is_empty() && sel.text(&us).is_none() && sel.range_in(1, 6).is_none());
    // Offsets past a unit's end are clamped.
    sel.set(1, 3..99);
    assert_eq!(sel.text(&us).as_deref(), Some("ond"));
}

#[test]
fn empty_cells_keep_their_tabs() {
    let doc = parse("| a | b | c |\n|--|--|--|\n| 1 |  | 3 |");
    let us = units(&doc, &[]);
    let mut sel = Selection::default();
    sel.select_all(&us);
    assert_eq!(sel.text(&us).as_deref(), Some("a\tb\tc\n1\t\t3"));
}

#[test]
fn word_ranges() {
    assert_eq!(word_range("foo bar_baz, qux", 5), 4..11);
    assert_eq!(word_range("foo bar_baz, qux", 11), 11..12);
    assert_eq!(word_range("привет мир", 2), 0..12);
}

#[test]
fn plain_text_strips_markdown() {
    let got = plain_text(
        "# Title\n\nSome **bold** and `code`.\n\n- one\n- two\n\n> quote\n\n```\nx = 1\n```\n\n| a | b |\n|--|--|\n| 1 | 2 |",
    );
    assert_eq!(
        got,
        "Title\nSome bold and code.\none\ntwo\nquote\nx = 1\na\tb\n1\t2"
    );
    assert_eq!(plain_text(""), "");
}

#[test]
fn first_paragraph_is_one_line() {
    assert_eq!(
        first_paragraph("# Head\n\nFirst **para**\nwraps here.\n\nSecond."),
        "First para wraps here."
    );
    assert_eq!(first_paragraph("- item one\n- item two"), "item one");
    assert_eq!(first_paragraph("# Only a heading"), "Only a heading");
    assert_eq!(first_paragraph("```\ncode\n```"), "");
}
