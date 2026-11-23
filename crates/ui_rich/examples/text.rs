//! Text editing preview: a `TextArea` in each submit mode, a `TextInput` in an input frame,
//! disabled fields, a long pre-filled text, and a live readout of events, selection and the
//! `@` / `/` token before the caret.
//!
//! `cargo run -p ui_rich --example text`

use gpui::{
    App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use ui::prelude::*;
use ui_rich::{
    SubmitOn, TextArea, TextAreaEvent, TextInput, TextInputEvent, text_area_field, text_field,
};

const TRIGGERS: &[char] = &['@', '/'];

const LONG: &str = "The reviewer agent reads the diff, runs the tests and writes a summary. \
It never pushes on its own: a human approves every merge.\n\nLong lines wrap at the field's \
width; past the maximum height the text scrolls and the caret stays in view. Try Up and Down \
across wrapped lines, double-click a word, triple-click a line, and drag past the bottom edge.\n\n\
Emoji 👨‍👩‍👧 and accents (é, ё) move as one character. Ctrl/Cmd+Z undoes a word at a time.";

struct Demo {
    areas: Vec<(&'static str, Entity<TextArea>)>,
    inputs: Vec<(&'static str, Entity<TextInput>)>,
    log: Vec<SharedString>,
    _subs: Vec<Subscription>,
}

impl Demo {
    fn new(cx: &mut Context<Self>) -> Self {
        let area = |cx: &mut Context<Self>, mode: SubmitOn, placeholder: &'static str| {
            cx.new(|cx| {
                TextArea::new(cx)
                    .submit_on(mode)
                    .placeholder(placeholder)
                    .rows(1, 6)
            })
        };
        let composer = area(
            cx,
            SubmitOn::Enter,
            "Message the agent… (@ to mention, / for commands)",
        );
        let comment = area(cx, SubmitOn::CmdEnter, "Leave a comment (Ctrl+Enter sends)");
        let notes = area(cx, SubmitOn::Never, "Notes (never submits)");
        let long = cx.new(|cx| TextArea::new(cx).rows(3, 8).with_text(LONG));
        let disabled = cx.new(|cx| TextArea::new(cx).with_text("Disabled text area"));
        disabled.update(cx, |a, cx| a.set_disabled(true, cx));
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search issues"));
        let off = cx.new(|cx| TextInput::new(cx).with_text("Disabled input"));
        off.update(cx, |i, cx| i.set_disabled(true, cx));

        let areas = vec![
            ("composer", composer),
            ("comment", comment),
            ("notes", notes),
            ("long", long),
            ("disabled area", disabled),
        ];
        let inputs = vec![("search", search), ("disabled input", off)];
        let mut subs = Vec::new();
        for (name, a) in &areas {
            let name = *name;
            subs.push(
                cx.subscribe(a, move |this: &mut Self, area, ev: &TextAreaEvent, cx| {
                    // The composer pretends to open a mention popup while a token is typed.
                    if name == "composer" && *ev == TextAreaEvent::Changed {
                        area.update(cx, |a, _| {
                            let open = a.token_before_cursor(TRIGGERS).is_some();
                            a.intercept_arrows(open);
                        });
                    }
                    if name == "composer"
                        && *ev == TextAreaEvent::Confirm
                        && let Some(t) = area.read(cx).token_before_cursor(TRIGGERS)
                    {
                        let pick = if t.trigger == '@' {
                            "@reviewer "
                        } else {
                            "/model "
                        };
                        area.update(cx, |a, cx| {
                            a.replace_token(&t, pick, cx);
                            a.intercept_arrows(false);
                        });
                    }
                    this.push(format!("{name}: {ev:?}"), cx);
                }),
            );
        }
        // The readout shows caret moves too, which emit no event.
        for (_, a) in &areas {
            subs.push(cx.observe(a, |_, _, cx| cx.notify()));
        }
        for (_, i) in &inputs {
            subs.push(cx.observe(i, |_, _, cx| cx.notify()));
        }
        for (name, i) in &inputs {
            let name = *name;
            subs.push(
                cx.subscribe(i, move |this: &mut Self, _, ev: &TextInputEvent, cx| {
                    this.push(format!("{name}: {ev:?}"), cx);
                }),
            );
        }
        Self {
            areas,
            inputs,
            log: Vec::new(),
            _subs: subs,
        }
    }

    fn push(&mut self, line: String, cx: &mut Context<Self>) {
        // Changed events come with every key; keep the readout readable.
        if self.log.last().is_some_and(|l| l.as_ref() == line) {
            return;
        }
        self.log.push(line.into());
        if self.log.len() > 14 {
            self.log.remove(0);
        }
        cx.notify();
    }

    fn readout(&self, window: &Window, cx: &App) -> Vec<SharedString> {
        let mut out = Vec::new();
        for (name, a) in &self.areas {
            let a = a.read(cx);
            if a.is_focused(window) {
                out.push(format!("focused: {name}").into());
                out.push(format!("selection: {:?}  caret: {}", a.selection(), a.cursor()).into());
                out.push(format!("token: {:?}", a.token_before_cursor(TRIGGERS)).into());
                out.push(format!("caret bounds: {:?}", a.caret_bounds()).into());
            }
        }
        for (name, i) in &self.inputs {
            let i = i.read(cx);
            if i.is_focused(window) {
                out.push(format!("focused: {name}").into());
                out.push(format!("selection: {:?}  text: {:?}", i.selection(), i.text()).into());
            }
        }
        if out.is_empty() {
            out.push("nothing focused".into());
        }
        out
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let labels = [
            "Composer: Enter sends, Shift+Enter is a newline",
            "Comment: Ctrl/Cmd+Enter sends",
            "Notes: never sends",
            "Long text: 3 to 8 rows",
            "Disabled",
        ];
        let areas = self
            .areas
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(ix, ((_, a), label))| {
                Field::new(label)
                    .child(text_area_field(("area", ix), a))
                    .into_any_element()
            });
        let readout = self.readout(window, cx);
        row()
            .size_full()
            .items_start()
            .gap(px(SPACE_6))
            .p(px(SPACE_6))
            .child(
                col()
                    .id("fields")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .gap(px(SPACE_4))
                    .child(
                        Field::new("Search").child(
                            text_field("search", &self.inputs[0].1)
                                .icon(IconName::Search)
                                .kbd("⌘K"),
                        ),
                    )
                    .child(Field::new("Disabled input").child(text_field("off", &self.inputs[1].1)))
                    .children(areas),
            )
            .child(
                col()
                    .w(px(PANEL_WIDTH))
                    .gap(px(SPACE_2))
                    .child(div().type_style(HEADING).child("State"))
                    .children(
                        readout
                            .into_iter()
                            .map(|l| div().type_style(MONO).text_color(TEXT_SECONDARY).child(l)),
                    )
                    .child(div().type_style(HEADING).pt(px(SPACE_4)).child("Events"))
                    .children(self.log.iter().map(|l| {
                        div()
                            .type_style(MONO)
                            .text_color(TEXT_MUTED)
                            .child(l.clone())
                    })),
            )
    }
}

fn main() {
    ui::run_app("Text editing", 1280., 860., |_, cx| {
        ui_rich::bind_keys(cx);
        cx.new(Demo::new)
    });
}
