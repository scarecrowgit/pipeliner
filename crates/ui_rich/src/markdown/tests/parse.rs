use std::sync::Arc;

use super::{SAMPLE, assert_well_formed};
use crate::markdown::cache::ParseCache;
use crate::markdown::parse::{Align, Block, Inline, Style, parse, parse_appended};

fn plain(s: &str) -> Inline {
    Inline {
        text: s.into(),
        style: Style::default(),
        link: None,
        image: false,
    }
}

fn texts(runs: &[Inline]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

#[test]
fn paragraphs_and_inline_styles() {
    let doc = parse("Hello **bold** *it* ~~gone~~ `code` [link](http://x).\nnext\n\nSecond.");
    assert_eq!(doc.blocks.len(), 2);
    let Block::Paragraph(runs) = &doc.blocks[0] else {
        panic!("{doc:?}")
    };
    assert_eq!(runs[0], plain("Hello "));
    let find = |t: &str| {
        runs.iter()
            .find(|r| r.text == t)
            .unwrap_or_else(|| panic!("{t} in {runs:?}"))
    };
    assert!(find("bold").style.bold);
    assert!(find("it").style.italic);
    assert!(find("gone").style.strike);
    assert!(find("code").style.code);
    assert_eq!(find("link").link.as_deref(), Some("http://x"));
    assert!(
        texts(runs).ends_with(".\nnext"),
        "soft break kept: {runs:?}"
    );
    assert_eq!(doc.blocks[1], Block::Paragraph(vec![plain("Second.")]));
}

#[test]
fn headings_one_to_four() {
    let doc = parse("# A\n## B\n### C\n#### D\n##### E\n###### F");
    let levels: Vec<u8> = doc
        .blocks
        .iter()
        .map(|b| match b {
            Block::Heading { level, .. } => *level,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(levels, [1, 2, 3, 4, 4, 4]);
    assert_eq!(
        doc.blocks[0],
        Block::Heading {
            level: 1,
            text: vec![plain("A")]
        }
    );
}

#[test]
fn nested_and_numbered_lists() {
    let doc = parse("3. one\n4. two\n   - a\n   - b\n5. three");
    let Block::List(list) = &doc.blocks[0] else {
        panic!("{doc:?}")
    };
    assert_eq!(list.start, Some(3));
    assert_eq!(list.items.len(), 3);
    assert_eq!(
        list.items[0].blocks,
        vec![Block::Paragraph(vec![plain("one")])]
    );
    let Block::List(sub) = &list.items[1].blocks[1] else {
        panic!("{list:?}")
    };
    assert_eq!(sub.start, None);
    assert_eq!(sub.items.len(), 2);
}

#[test]
fn task_lists() {
    let doc = parse("- [x] done\n- [ ] open\n- plain");
    let Block::List(list) = &doc.blocks[0] else {
        panic!("{doc:?}")
    };
    let tasks: Vec<_> = list.items.iter().map(|i| i.task).collect();
    assert_eq!(tasks, [Some(true), Some(false), None]);
    assert_eq!(
        list.items[0].blocks,
        vec![Block::Paragraph(vec![plain("done")])]
    );
}

#[test]
fn quotes_hold_blocks() {
    let doc = parse("> quoted **b**\n>\n> - item");
    let Block::Quote(inner) = &doc.blocks[0] else {
        panic!("{doc:?}")
    };
    assert!(matches!(inner[0], Block::Paragraph(_)));
    assert!(matches!(inner[1], Block::List(_)));
}

#[test]
fn code_blocks_keep_language_and_text() {
    let doc = parse("```rust title\nfn main() {}\n```\n\n~~~\nplain\n~~~\n\n    indented\n");
    assert_eq!(
        doc.blocks[0],
        Block::Code {
            lang: "rust".into(),
            text: "fn main() {}".into()
        }
    );
    assert_eq!(
        doc.blocks[1],
        Block::Code {
            lang: String::new(),
            text: "plain".into()
        }
    );
    assert_eq!(
        doc.blocks[2],
        Block::Code {
            lang: String::new(),
            text: "indented".into()
        }
    );
}

#[test]
fn tables_are_rectangular() {
    let doc = parse("| a | b | c |\n|:--|--:|:-:|\n| 1 | 2 |\n| x | y | z | extra |");
    let Block::Table(t) = &doc.blocks[0] else {
        panic!("{doc:?}")
    };
    assert_eq!(t.align, [Align::Left, Align::Right, Align::Center]);
    assert_eq!(t.head.len(), 3);
    assert_eq!(t.rows.len(), 2);
    assert!(t.rows.iter().all(|r| r.len() == 3));
    assert_eq!(t.rows[0][2], Vec::<Inline>::new());
    assert_eq!(texts(&t.rows[1][2]), "z");
}

#[test]
fn rules_and_empty_input() {
    let doc = parse("a\n\n---\n\nb");
    assert_eq!(doc.blocks[1], Block::Rule);
    assert!(parse("").blocks.is_empty());
    assert!(parse("  \n\n ").blocks.is_empty());
}

#[test]
fn unclosed_fence_is_code_to_the_end() {
    let doc = parse("Look:\n\n```py\nprint(1)\nprint(2");
    assert_eq!(doc.blocks.len(), 2);
    assert_eq!(
        doc.blocks[1],
        Block::Code {
            lang: "py".into(),
            text: "print(1)\nprint(2".into()
        }
    );
}

#[test]
fn half_written_table_and_list() {
    let doc = parse("| a | b |\n|--");
    assert_well_formed(&doc, "half table");
    assert!(!doc.blocks.is_empty());
    let doc = parse("- one\n- ");
    let Block::List(list) = &doc.blocks[0] else {
        panic!("{doc:?}")
    };
    assert_eq!(list.items.len(), 2);
}

#[test]
fn every_prefix_parses_into_a_well_formed_tree() {
    let mut cuts: Vec<usize> = SAMPLE.char_indices().map(|(i, _)| i).collect();
    cuts.push(SAMPLE.len());
    for &i in &cuts {
        let src = &SAMPLE[..i];
        assert_well_formed(&parse(src), src);
    }
}

#[test]
fn streaming_reparse_matches_a_full_parse() {
    let mut cuts: Vec<usize> = SAMPLE.char_indices().map(|(i, _)| i).collect();
    cuts.push(SAMPLE.len());
    let mut prev_text = "";
    let mut prev = parse("");
    for &i in &cuts {
        let src = &SAMPLE[..i];
        let doc = parse_appended(&prev, prev_text, src);
        let full = parse(src);
        if doc.blocks != full.blocks {
            let k = doc
                .blocks
                .iter()
                .zip(&full.blocks)
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            let tail = &src[src.len().saturating_sub(120)..];
            panic!(
                "after ...{tail:?}\nstreamed {:#?}\nfull {:#?}",
                &doc.blocks[k.min(doc.blocks.len())..],
                &full.blocks[k.min(full.blocks.len())..]
            );
        }
        assert_well_formed(&doc, src);
        prev = doc;
        prev_text = src;
    }
}

#[test]
fn cache_parses_a_text_once_and_replaces_stream_stages() {
    let mut cache = ParseCache::new(8);
    let a = cache.get("# Hi\n\nthere", None);
    let b = cache.get("# Hi\n\nthere", None);
    assert!(Arc::ptr_eq(&a, &b));
    let c = cache.get("# Hi\n\nthere and more", Some(("# Hi\n\nthere", &a)));
    assert_eq!(c.blocks, parse("# Hi\n\nthere and more").blocks);
    assert_eq!(cache.len(), 1, "the stream stage was replaced");
    for i in 0..20 {
        cache.get(&format!("text {i}"), None);
    }
    assert_eq!(cache.len(), 8);
}
