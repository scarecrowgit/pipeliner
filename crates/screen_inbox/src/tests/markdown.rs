//! A report body with bold labels at the start of list items: only the label is bold.

use ui_rich::markdown::{Block, parse};

const REPORT: &str = "Findings for HAR-4.\n\n- **Triage (Decision):** yes · the crash reproduces on main\n- **Owner:** the payments team · they own the retry path\n  and the webhook handler\n- plain item with **one** bold word\n";

#[test]
fn only_the_label_of_a_list_item_is_bold() {
    let doc = parse(REPORT);
    let Some(Block::List(list)) = doc.blocks.iter().find(|b| matches!(b, Block::List(_))) else {
        panic!("no list in {:?}", doc.blocks);
    };
    let runs = |ix: usize| -> Vec<(String, bool)> {
        match list.items[ix].blocks.first() {
            Some(Block::Paragraph(inlines)) => inlines
                .iter()
                .map(|i| (i.text.clone(), i.style.bold))
                .collect(),
            other => panic!("item {ix} is {other:?}"),
        }
    };
    let first = runs(0);
    assert!(
        first
            .iter()
            .any(|(t, b)| *b && t.contains("Triage (Decision):")),
        "{first:?}"
    );
    assert!(
        first
            .iter()
            .any(|(t, b)| !*b && t.contains("yes · the crash")),
        "{first:?}"
    );
    let second = runs(1);
    assert!(
        second
            .iter()
            .any(|(t, b)| !*b && t.contains("the payments team")),
        "{second:?}"
    );
    let third = runs(2);
    assert!(
        third.iter().any(|(t, b)| !*b && t.contains("plain item")),
        "{third:?}"
    );
}
