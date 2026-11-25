//! Markdown tests: the parser and streaming, the text model and selection (GPUI-free), and
//! the view in a test window.

mod group;
mod image_window;
mod images;
mod parse;
mod runs;
mod text;
mod verbatim;
mod window;

use super::parse::{Block, Document, Inline};

/// A document with every block and inline type, for the prefix and streaming tests.
pub(super) const SAMPLE: &str = concat!(
    r#"# Retry budget

The **gateway** drops a *duplicate* charge when retries reuse one ~~random~~ key: see `retryCharge` in [the docs](https://example.com/docs). Ask @Fixer or @Codex.
Second line of the same paragraph.

## Steps

1. Find every caller
2. Add the key
   - derive it from the charge
   - keep it across retries
3. Write a test

- [x] done item
- [ ] open item

> A quote with **bold** text.
>
> - and a list inside

```ts
const key = charge.idempotencyKey ?? deriveKey(charge);
"#,
    "\treturn retryCharge({ ...charge, idempotencyKey: key });\n",
    r#"```

| Variant | Lines | Tests |
|:--|--:|:-:|
| A | +18 −4 | 212 of 212 |
| B | +31 |

---

### Third level
#### Fourth level
##### Fifth level

    indented code

Last paragraph with a hard  
break and ![an image](https://example.com/i.png).
"#
);

/// Panics with a description when `doc` breaks an invariant the renderer relies on.
pub(super) fn assert_well_formed(doc: &Document, src: &str) {
    assert_eq!(doc.starts.len(), doc.blocks.len(), "starts for {src:?}");
    assert!(
        doc.starts.windows(2).all(|w| w[0] <= w[1]),
        "starts order for {src:?}"
    );
    for b in &doc.blocks {
        check_block(b, src);
    }
}

fn check_runs(runs: &[Inline], src: &str, allow_empty: bool) {
    assert!(
        allow_empty || !runs.is_empty(),
        "empty text block in {src:?}"
    );
    for r in runs {
        assert!(r.image || !r.text.is_empty(), "empty run in {src:?}");
    }
    for w in runs.windows(2) {
        assert!(
            w[0].style != w[1].style || w[0].link != w[1].link || w[0].image || w[1].image,
            "unmerged runs in {src:?}"
        );
    }
}

fn check_block(b: &Block, src: &str) {
    match b {
        Block::Paragraph(runs) => check_runs(runs, src, false),
        Block::Heading { level, text } => {
            assert!((1..=4).contains(level), "heading level in {src:?}");
            check_runs(text, src, false);
        }
        Block::List(l) => {
            for item in &l.items {
                item.blocks.iter().for_each(|b| check_block(b, src));
            }
        }
        Block::Quote(blocks) => blocks.iter().for_each(|b| check_block(b, src)),
        Block::Code { text, .. } => assert!(!text.ends_with('\n'), "code newline in {src:?}"),
        Block::Table(t) => {
            assert_eq!(t.align.len(), t.head.len(), "align in {src:?}");
            for row in &t.rows {
                assert_eq!(row.len(), t.head.len(), "row width in {src:?}");
            }
            t.head
                .iter()
                .chain(t.rows.iter().flatten())
                .for_each(|c| check_runs(c, src, true));
        }
        Block::Rule => {}
    }
}
