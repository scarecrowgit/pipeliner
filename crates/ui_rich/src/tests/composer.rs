//! The composer in a test window under an overlay host: send keys, attachments, trigger
//! detection, the completion menu, mentions, busy and disabled.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    ClipboardItem, Context, Entity, Image, ImageFormat, IntoElement, Modifiers, Render,
    Subscription, TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};
use ui::OverlayHost;
use ui::overlay::overlays;

use super::{Log, clear};
use crate::{
    Attachment, AttachmentLimits, Completion, Composer, ComposerEvent as Ev, Draft, Trigger,
    TriggerKind,
};

/// A page with room above the composer, so the menu opens over the page.
struct Page {
    composer: Entity<Composer>,
    _sub: Subscription,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .flex()
            .flex_col()
            .child(div().h(px(500.)))
            .child(div().w(px(640.)).child(self.composer.clone()))
    }
}

/// Open a window with the composer `make` builds, focused; return it and its event log.
fn open(
    cx: &mut TestAppContext,
    make: impl FnOnce(&mut Context<Composer>) -> Composer + 'static,
) -> (Entity<Composer>, Log<Ev>, &mut VisualTestContext) {
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        crate::bind_keys(cx);
    });
    let log: Log<Ev> = Rc::default();
    let l = log.clone();
    let slot: Rc<RefCell<Option<Entity<Composer>>>> = Rc::default();
    let s = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let composer = cx.new(make);
        let sub = cx.subscribe(&composer, move |_, _, e: &Ev, _| l.borrow_mut().push(e.clone()));
        *s.borrow_mut() = Some(composer.clone());
        let page = cx.new(|_| Page { composer, _sub: sub });
        OverlayHost::new(window, cx).with_content(page)
    });
    let Some(composer) = slot.borrow_mut().take() else {
        panic!("composer not built")
    };
    cx.update(|window, cx| {
        window.activate_window();
        composer.read(cx).focus(window, cx);
    });
    cx.run_until_parked();
    clear(&log);
    (composer, log, cx)
}

/// Type `text`; spaces go in as the space key.
fn typ(cx: &mut VisualTestContext, text: &str) {
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            cx.simulate_keystrokes("space");
        }
        if !word.is_empty() {
            cx.simulate_input(word);
        }
    }
}

fn text(c: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
    c.read_with(cx, |c, _| c.text().to_string())
}

fn submits(log: &Log<Ev>) -> Vec<(String, usize, usize)> {
    log.borrow()
        .iter()
        .filter_map(|e| match e {
            Ev::Submit { text, attachments, mentions } => {
                Some((text.clone(), attachments.len(), mentions.len()))
            }
            _ => None,
        })
        .collect()
}

fn triggers(log: &Log<Ev>) -> Vec<Option<Trigger>> {
    log.borrow()
        .iter()
        .filter_map(|e| match e {
            Ev::TriggerChanged(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn open_menus(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| overlays(window, cx).open_count(cx))
}

fn items() -> Vec<Completion> {
    vec![
        Completion::new("reproducer", "Reproducer", "@Reproducer")
            .detail("Agent · on this issue now")
            .icon(ui::assets::IconName::Agent)
            .hint("running"),
        Completion::new("release", "Release captain", "@Release-captain").hint("idle"),
        Completion::new("reviewer", "Reviewer", "@Reviewer"),
    ]
}

fn png() -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3, 4]))
}

#[gpui::test]
fn chat_enter_sends_and_shift_enter_is_a_newline(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    typ(cx, "fix it");
    cx.simulate_keystrokes("shift-enter");
    typ(cx, "now ");
    assert_eq!(text(&c, cx), "fix it\nnow ");
    assert!(submits(&log).is_empty());
    cx.simulate_keystrokes("enter");
    assert_eq!(submits(&log), vec![("fix it\nnow".to_string(), 0, 0)]);
    assert_eq!(text(&c, cx), "", "the composer clears after sending");
    assert_eq!(log.borrow().last(), Some(&Ev::DraftChanged));
}

#[gpui::test]
fn issue_enter_is_a_newline_and_ctrl_enter_sends(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::issue);
    typ(cx, "a");
    cx.simulate_keystrokes("enter");
    typ(cx, "b");
    assert_eq!(text(&c, cx), "a\nb");
    assert!(submits(&log).is_empty());
    cx.simulate_keystrokes("ctrl-enter");
    assert_eq!(submits(&log), vec![("a\nb".to_string(), 0, 0)]);
}

#[gpui::test]
fn empty_or_blank_text_is_not_sent(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    cx.simulate_keystrokes("enter");
    typ(cx, "   ");
    cx.simulate_keystrokes("shift-enter enter");
    assert!(submits(&log).is_empty());
    c.read_with(cx, |c, _| assert!(!c.can_send()));
    let sent = c.update(cx, |c, cx| c.submit(cx));
    assert!(!sent);
    // A click on the disabled Send does nothing either.
    let send = cx.debug_bounds("composer-send").expect("send button drawn");
    cx.simulate_click(send.center(), Modifiers::none());
    assert!(submits(&log).is_empty());
}

#[gpui::test]
fn the_send_button_sends(cx: &mut TestAppContext) {
    let (_, log, cx) = open(cx, Composer::chat);
    typ(cx, "hi");
    cx.run_until_parked();
    let send = cx.debug_bounds("composer-send").expect("send button drawn");
    cx.simulate_click(send.center(), Modifiers::none());
    assert_eq!(submits(&log), vec![("hi".to_string(), 0, 0)]);
}

#[gpui::test]
fn attachments_are_added_removed_and_sent(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    let a = c.update(cx, |c, cx| c.add_attachment(Attachment::image("retry-log.png", png()), cx));
    let Ok(a) = a else { panic!("refused: {a:?}") };
    let file = Attachment::file("notes.txt", "/tmp/notes.txt".into(), 2_000);
    let b = c.update(cx, |c, cx| c.add_attachment(file, cx)).expect("added");
    assert_ne!(a, b);
    assert_eq!(log.borrow().iter().filter(|e| **e == Ev::DraftChanged).count(), 2);

    // Attachments alone can be sent.
    c.read_with(cx, |c, _| assert!(c.can_send()));
    c.update(cx, |c, cx| c.remove_attachment(b, cx));
    c.read_with(cx, |c, _| assert_eq!(c.attachments().len(), 1));
    cx.simulate_keystrokes("enter");
    assert_eq!(submits(&log), vec![(String::new(), 1, 0)]);
    c.read_with(cx, |c, _| assert!(c.attachments().is_empty()));
}

#[gpui::test]
fn uploads_hold_the_send_and_limits_refuse(cx: &mut TestAppContext) {
    let limits = AttachmentLimits { max_count: 2, max_size: 10 };
    let (c, log, cx) = open(cx, move |cx| Composer::chat(cx).limits(limits));
    let big = Attachment::file("big.bin", "/tmp/big.bin".into(), 11);
    let err = c.update(cx, |c, cx| c.add_attachment(big, cx)).expect_err("too large");
    assert_eq!(err.as_ref(), "big.bin is 11 B · 10 B max each");
    let up = Attachment::image("a.png", png()).uploading(0.5);
    let id = c.update(cx, |c, cx| c.add_attachment(up, cx)).expect("added");
    c.update(cx, |c, cx| c.add_attachment(Attachment::image("b.png", png()), cx)).expect("added");
    let err = c.update(cx, |c, cx| c.add_attachment(Attachment::image("c.png", png()), cx));
    assert_eq!(
        err.expect_err("full").as_ref(),
        "c.png not added · 2 of 2 images already · remove one first"
    );
    typ(cx, "go");
    cx.simulate_keystrokes("enter");
    assert!(submits(&log).is_empty(), "nothing goes out while uploading");
    c.update(cx, |c, cx| c.set_attachment_progress(id, None, cx));
    cx.simulate_keystrokes("enter");
    assert_eq!(submits(&log), vec![("go".to_string(), 2, 0)]);
}

#[gpui::test]
fn pasted_images_become_attachments(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    let image = Image::from_bytes(ImageFormat::Png, vec![9, 9, 9]);
    cx.write_to_clipboard(ClipboardItem::new_image(&image));
    cx.simulate_keystrokes("ctrl-v");
    c.read_with(cx, |c, _| {
        let names: Vec<_> = c.attachments().iter().map(|a| a.name.to_string()).collect();
        assert_eq!(names, ["pasted-image-1.png"]);
        assert_eq!(c.attachments()[0].size, 3);
        assert!(c.text().is_empty());
    });
    assert!(log.borrow().contains(&Ev::DraftChanged));
}

#[gpui::test]
fn backspace_in_an_empty_composer_removes_the_last_chip(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    let a = c.update(cx, |c, cx| c.add_attachment(Attachment::image("a.png", png()), cx)).expect("a");
    let b = c.update(cx, |c, cx| c.add_attachment(Attachment::image("b.png", png()), cx)).expect("b");
    typ(cx, "x");
    cx.simulate_keystrokes("backspace");
    c.read_with(cx, |c, _| assert_eq!(c.attachments().len(), 2));
    clear(&log);
    cx.simulate_keystrokes("backspace");
    c.read_with(cx, |c, _| assert_eq!(c.attachments().iter().map(|x| x.id).collect::<Vec<_>>(), [a]));
    assert!(log.borrow().contains(&Ev::AttachmentRemoved(b)));
}

#[gpui::test]
fn a_trigger_needs_a_word_start(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    typ(cx, "a@b");
    assert!(triggers(&log).is_empty(), "@ inside a word does not trigger");
    typ(cx, " @b");
    let mention = |q: &str| Some(Trigger { kind: TriggerKind::Mention, query: q.into(), start: 4 });
    assert_eq!(triggers(&log), vec![mention(""), mention("b")]);
    c.read_with(cx, |c, _| assert_eq!(c.trigger(), mention("b").as_ref()));
    clear(&log);
    typ(cx, " ");
    assert_eq!(triggers(&log), vec![None]);
    clear(&log);
    typ(cx, "/mo");
    let last = triggers(&log).pop().flatten().expect("command trigger");
    assert_eq!((last.kind, last.query.as_str(), last.start), (TriggerKind::Command, "mo", 7));
}

#[gpui::test]
fn the_issue_composer_only_mentions(cx: &mut TestAppContext) {
    let (_, log, cx) = open(cx, Composer::issue);
    typ(cx, "/model");
    assert!(triggers(&log).is_empty());
    typ(cx, " @fi");
    assert_eq!(triggers(&log).last().cloned().flatten().map(|t| t.query), Some("fi".into()));
}

#[gpui::test]
fn the_menu_moves_and_accepts_into_a_mention(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    typ(cx, "ask @re");
    c.update(cx, |c, cx| c.set_completions(items(), Some(8), cx));
    cx.run_until_parked();
    assert_eq!(open_menus(cx), 1, "the menu opened in the overlay host");
    c.read_with(cx, |c, _| assert_eq!(c.selected(), Some(0)));
    cx.update(|window, cx| assert!(c.read(cx).is_focused(window, cx), "typing stays in the text"));
    cx.simulate_keystrokes("down down");
    c.read_with(cx, |c, _| assert_eq!(c.selected(), Some(2)));
    cx.simulate_keystrokes("down");
    c.read_with(cx, |c, _| assert_eq!(c.selected(), Some(0), "wraps"));
    cx.simulate_keystrokes("up");
    c.read_with(cx, |c, _| assert_eq!(c.selected(), Some(2)));
    assert_eq!(text(&c, cx), "ask @re", "arrows did not move the caret or edit");
    clear(&log);
    cx.simulate_keystrokes("enter");
    assert_eq!(text(&c, cx), "ask @Reviewer ");
    assert!(submits(&log).is_empty(), "Enter picked, it did not send");
    assert_eq!(triggers(&log), vec![None]);
    cx.run_until_parked();
    assert_eq!(open_menus(cx), 0, "the menu closed");
    c.read_with(cx, |c, _| {
        let m = &c.mentions()[0];
        assert_eq!((m.id.as_ref(), m.text.as_ref(), m.range.clone()), ("reviewer", "@Reviewer", 4..13));
    });
    typ(cx, "please");
    cx.simulate_keystrokes("enter");
    let sent = log.borrow().iter().find_map(|e| match e {
        Ev::Submit { text, mentions, .. } => Some((text.clone(), mentions.clone())),
        _ => None,
    });
    let Some((sent, mentions)) = sent else { panic!("not sent") };
    assert_eq!(sent, "ask @Reviewer please");
    assert_eq!(&sent[mentions[0].range.clone()], "@Reviewer");
}

#[gpui::test]
fn tab_accepts_and_spans_move_with_edits(cx: &mut TestAppContext) {
    let (c, _, cx) = open(cx, Composer::chat);
    typ(cx, "@re");
    c.update(cx, |c, cx| c.set_completions(items(), None, cx));
    cx.simulate_keystrokes("tab");
    assert_eq!(text(&c, cx), "@Reproducer ");
    // Typing before the mention shifts it.
    cx.simulate_keystrokes("home");
    typ(cx, "hey ");
    c.read_with(cx, |c, _| {
        assert_eq!(c.text(), "hey @Reproducer ");
        assert_eq!(c.mentions()[0].range, 4..15);
    });
    // Typing inside it turns it back into plain text.
    cx.simulate_keystrokes("right right");
    typ(cx, "x");
    c.read_with(cx, |c, _| assert!(c.mentions().is_empty()));
}

#[gpui::test]
fn backspace_deletes_a_mention_whole(cx: &mut TestAppContext) {
    let (c, _, cx) = open(cx, Composer::chat);
    typ(cx, "ask @re");
    c.update(cx, |c, cx| c.set_completions(items(), None, cx));
    cx.simulate_keystrokes("enter");
    cx.simulate_keystrokes("backspace");
    assert_eq!(text(&c, cx), "ask @Reproducer");
    c.read_with(cx, |c, _| {
        assert_eq!(c.mentions().len(), 1);
        assert_eq!(c.trigger(), None, "an accepted mention does not reopen the menu");
    });
    cx.simulate_keystrokes("backspace");
    assert_eq!(text(&c, cx), "ask ");
    c.read_with(cx, |c, _| assert!(c.mentions().is_empty()));
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&c, cx), "ask @Reproducer", "one undo step brings it back as text");
}

#[gpui::test]
fn escape_closes_the_menu_until_the_trigger_is_retyped(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    typ(cx, "@r");
    c.update(cx, |c, cx| c.set_completions(items(), None, cx));
    cx.run_until_parked();
    assert_eq!(open_menus(cx), 1);
    clear(&log);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(open_menus(cx), 0);
    assert_eq!(triggers(&log), vec![None]);
    c.read_with(cx, |c, _| assert!(!c.menu_open()));
    typ(cx, "e");
    assert_eq!(triggers(&log), vec![None], "stays closed for this trigger");
    // Enter now sends, since no menu takes it.
    cx.simulate_keystrokes("enter");
    assert_eq!(submits(&log), vec![("@re".to_string(), 0, 0)]);
    clear(&log);
    typ(cx, "@");
    assert_eq!(triggers(&log).len(), 1, "a new trigger opens again");
}

#[gpui::test]
fn busy_shows_stop_and_holds_the_send(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    typ(cx, "next");
    c.update(cx, |c, cx| c.set_busy(true, cx));
    cx.run_until_parked();
    let stop = cx.debug_bounds("composer-stop").expect("stop button drawn");
    cx.simulate_keystrokes("enter");
    assert!(submits(&log).is_empty());
    cx.simulate_click(stop.center(), Modifiers::none());
    assert_eq!(log.borrow().iter().filter(|e| **e == Ev::Stop).count(), 1);
    cx.update(|window, cx| c.read(cx).focus(window, cx));
    cx.simulate_keystrokes("escape");
    assert_eq!(log.borrow().iter().filter(|e| **e == Ev::Stop).count(), 2);
    c.update(cx, |c, cx| c.set_busy(false, cx));
    cx.simulate_keystrokes("enter");
    assert_eq!(submits(&log), vec![("next".to_string(), 0, 0)]);
}

#[gpui::test]
fn a_disabled_composer_takes_nothing(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    c.update(cx, |c, cx| c.set_disabled(Some("Coming soon".into()), cx));
    cx.run_until_parked();
    typ(cx, "x @a");
    cx.simulate_keystrokes("enter");
    c.read_with(cx, |c, _| {
        assert!(c.text().is_empty());
        assert!(!c.can_send());
        assert_eq!(c.disabled_reason().map(|r| r.as_ref()), Some("Coming soon"));
    });
    assert!(submits(&log).is_empty());
    assert!(triggers(&log).is_empty());
    c.update(cx, |c, cx| c.set_disabled(None, cx));
    cx.update(|window, cx| c.read(cx).focus(window, cx));
    typ(cx, "ok");
    cx.simulate_keystrokes("enter");
    assert_eq!(submits(&log), vec![("ok".to_string(), 0, 0)]);
}

#[gpui::test]
fn drafts_report_changes_and_restore(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    c.update(cx, |c, cx| c.set_text("saved", cx));
    assert!(!log.borrow().contains(&Ev::DraftChanged), "set_text is not a user edit");
    typ(cx, "!");
    assert!(log.borrow().contains(&Ev::DraftChanged));
    let mut draft = c.read_with(cx, |c, _| c.draft());
    assert_eq!(draft.text, "saved!");
    draft.mentions.push(crate::Mention {
        kind: TriggerKind::Mention,
        id: "x".into(),
        text: "@gone".into(),
        range: 0..5,
    });
    c.update(cx, |c, cx| c.restore(Draft { text: "@Fixer hi".into(), ..draft.clone() }, cx));
    c.read_with(cx, |c, _| {
        assert_eq!(c.text(), "@Fixer hi");
        assert!(c.mentions().is_empty(), "a span that does not read its text is dropped");
    });
}

#[gpui::test]
fn a_click_picks_a_row_and_a_click_outside_closes(cx: &mut TestAppContext) {
    let (c, log, cx) = open(cx, Composer::chat);
    typ(cx, "@re");
    c.update(cx, |c, cx| c.set_completions(items(), Some(3), cx));
    cx.run_until_parked();
    let row = cx.debug_bounds("composer-item-1").expect("row drawn");
    cx.simulate_click(row.center(), Modifiers::none());
    assert_eq!(text(&c, cx), "@Release-captain ");
    cx.update(|window, cx| assert!(c.read(cx).is_focused(window, cx), "focus is back in the text"));
    cx.run_until_parked();
    assert_eq!(open_menus(cx), 0);

    typ(cx, "@r");
    c.update(cx, |c, cx| c.set_completions(items(), None, cx));
    cx.run_until_parked();
    assert_eq!(open_menus(cx), 1);
    clear(&log);
    cx.simulate_click(gpui::point(px(5.), px(5.)), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(open_menus(cx), 0);
    c.read_with(cx, |c, _| assert!(!c.menu_open(), "the composer saw the menu close"));
    assert_eq!(triggers(&log), vec![None]);
}

#[gpui::test]
#[allow(clippy::single_range_in_vec_init)]
fn accepted_mentions_are_drawn_highlighted_and_follow_edits(cx: &mut TestAppContext) {
    let (c, _, cx) = open(cx, |cx| Composer::chat(cx).rows(2, 6, cx));
    let styled = |c: &Entity<Composer>, cx: &mut VisualTestContext| {
        c.read_with(cx, |c, cx| {
            c.text_area().read(cx).styles().iter().map(|s| s.range.clone()).collect::<Vec<_>>()
        })
    };
    typ(cx, "ask @re");
    c.update(cx, |c, cx| c.set_completions(items(), Some(8), cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text(&c, cx), "ask @Reproducer ");
    assert_eq!(styled(&c, cx), [4..15], "the mention is highlighted");
    c.read_with(cx, |c, cx| {
        let s = &c.text_area().read(cx).styles()[0];
        assert_eq!((s.color, s.background), (Some(ui::theme::ACCENT_TEXT), Some(ui::theme::ACCENT_SOFT)));
    });
    // Typing before the mention moves the highlight with it.
    cx.simulate_keystrokes("ctrl-home");
    typ(cx, "so ");
    cx.run_until_parked();
    assert_eq!(styled(&c, cx), [7..18]);
    // Typing into it makes it plain text again.
    cx.simulate_keystrokes("ctrl-end left left");
    typ(cx, "x");
    cx.run_until_parked();
    assert!(styled(&c, cx).is_empty(), "{:?}", styled(&c, cx));
    c.update(cx, |c, cx| c.set_rows(1, 3, cx));
}

/// An issue composer in a frame of a known width, as the issue panel draws it.
struct NarrowPage {
    composer: Entity<Composer>,
    width: f32,
}

impl Render for NarrowPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let composer = crate::composer_view(&self.composer).width(self.width);
        ui::root().flex().flex_col().child(div().w(px(self.width)).child(composer))
    }
}

#[gpui::test]
fn a_long_hint_is_cut_before_the_send_button_leaves_the_card(cx: &mut TestAppContext) {
    const WIDTH: f32 = 300.;
    ui::motion::set_reduced(true);
    cx.update(|cx| {
        ui::init(cx);
        crate::bind_keys(cx);
    });
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let composer = cx.new(|cx| {
            let mut c = Composer::issue(cx);
            c.set_hint(Some("Mentions wake agents when you post, and the hint goes on and on".into()), cx);
            c
        });
        let page = cx.new(|_| NarrowPage { composer, width: WIDTH });
        OverlayHost::new(window, cx).with_content(page)
    });
    cx.run_until_parked();
    let send = cx.debug_bounds("composer-send").expect("send button drawn");
    let hint = cx.debug_bounds("composer-hint").expect("hint drawn");
    assert!(send.right() <= px(WIDTH), "the button stays inside the card: {send:?}");
    assert!(hint.right() <= send.left(), "the hint ends before the button: {hint:?} {send:?}");
    assert!(hint.size.width > px(40.), "the hint keeps the room that is left: {hint:?}");
}
