//! Following the stream, scrolling away from it, jumping back, and loading older pages.

use gpui::{ListOffset, TestAppContext, VisualTestContext, px};
use hub::model::AgentEvent;

use super::{LEDGER, Open, click, counts, drawn, emit, open, settle, wheel};

fn following(open: &Open, cx: &mut VisualTestContext) -> bool {
    open.view.read_with(cx, |view, _| view.is_following())
}

fn top(open: &Open, cx: &mut VisualTestContext) -> (usize, f32) {
    open.view.read_with(cx, |view, _| {
        let top = view.list.logical_scroll_top();
        (top.item_ix, f32::from(top.offset_in_item))
    })
}

#[gpui::test]
fn a_streamed_delta_updates_its_row_and_keeps_following(cx: &mut TestAppContext) {
    let s = "chat-split-checkout";
    let (open, cx) = open(cx, s, 600.);
    assert!(following(&open, cx));
    emit(&open, cx, s, vec![AgentEvent::TurnStarted, AgentEvent::TextDelta { text: "Let me ".into() }]);
    let (rows, blocks) = counts(&open, cx);
    let index = blocks - 1;
    let (id, md) = open.view.read_with(cx, |view, cx| {
        let id = view.model.read(cx).blocks()[index].id;
        (id, view.markdown.get(&id).map(|md| md.entity_id()))
    });
    assert!(md.is_some(), "the streaming row was drawn");
    assert!(drawn(cx, "streaming-caret".into()) || drawn(cx, format!("block-{index}")));

    emit(&open, cx, s, vec![AgentEvent::TextDelta { text: "check the cart API.".into() }]);
    assert_eq!(counts(&open, cx), (rows, blocks), "the delta changed a row, it added none");
    open.view.read_with(cx, |view, cx| {
        assert_eq!(view.model.read(cx).blocks()[index].id, id, "the row keeps its key");
        let now = &view.markdown[&id];
        assert_eq!(Some(now.entity_id()), md, "the row keeps its Markdown view");
        assert_eq!(now.read(cx).text(), "Let me check the cart API.");
    });
    assert!(following(&open, cx));
}

#[gpui::test]
fn scrolling_up_stops_following_and_new_rows_do_not_move_the_view(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, LEDGER, 600.);
    assert!(following(&open, cx));
    assert!(!drawn(cx, "jump-latest".into()));
    wheel(cx, 400.);
    assert!(!following(&open, cx), "a scroll up stops following");
    assert!(drawn(cx, "jump-latest".into()));
    let before = top(&open, cx);

    emit(
        &open,
        cx,
        LEDGER,
        vec![AgentEvent::TurnStarted, AgentEvent::TextDelta { text: "A new answer arrives below.".into() }],
    );
    emit(&open, cx, LEDGER, vec![AgentEvent::TextDelta { text: " And grows.".into() }]);
    assert_eq!(top(&open, cx), before, "the view stays where the person put it");
    assert!(!following(&open, cx));

    click(cx, "jump-latest");
    assert!(following(&open, cx), "Jump to latest follows again");
}

#[gpui::test]
fn nearing_the_top_loads_the_older_page(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, LEDGER, 600.);
    let (_, loaded) = counts(&open, cx);
    assert!(open.view.read_with(cx, |view, cx| view.model.read(cx).has_older()));
    // Rows above the measured ones count as empty until drawn: keep turning the wheel, as a
    // person does, until the top is near and the older page comes in.
    for _ in 0..40 {
        wheel(cx, 2_000.);
        if counts(&open, cx).1 > loaded {
            break;
        }
    }
    let (rows, blocks) = counts(&open, cx);
    assert!(blocks > loaded, "the older page arrived: {loaded} -> {blocks}");
    assert_eq!(rows, blocks + 1);
}

#[gpui::test]
fn an_older_page_keeps_the_visible_rows_in_place(cx: &mut TestAppContext) {
    let (open, cx) = open(cx, LEDGER, 600.);
    let anchor = open.view.update(cx, |view, cx| {
        view.list.scroll_to(ListOffset { item_ix: 5, offset_in_item: px(7.) });
        view.model.read(cx).blocks()[5].id
    });
    settle(cx);
    open.view.update(cx, |view, cx| view.model.update(cx, |model, cx| model.load_older(cx)));
    settle(cx);
    let now = open.view.read_with(cx, |view, cx| {
        let top = view.list.logical_scroll_top();
        let blocks = view.model.read(cx).blocks();
        (top.item_ix > 5, blocks.get(top.item_ix).map(|block| block.id), f32::from(top.offset_in_item))
    });
    assert_eq!(now, (true, Some(anchor), 7.), "the row at the top stays at the top");
}
