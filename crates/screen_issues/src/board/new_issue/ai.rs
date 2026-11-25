//! "Write with AI" in the new-issue dialog (`new-issue-dialog.html`, `new-issue-ai-draft.html`):
//! the description box with its footer, the draft under review and the violet bar that keeps,
//! retries, changes or undoes it.

use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, FocusHandle, IntoElement, MouseButton, SharedString,
    Stateful, Subscription, Window, div, prelude::*, px,
};
use hub::api::{Attachment, DescriptionDraftRequest};
use ui::dev::Probe;
use ui::prelude::*;
use ui::tooltip::Tooltip;
use ui_rich::markdown::Markdown;
use ui_rich::{TextInput, TextInputEvent};

use super::NewIssueForm;
use crate::panel::Named as _;

/// Id and probe of the "Write with AI" button.
pub(crate) const WRITE: &str = "new-issue-ai";
/// Probe of the draft under review.
pub(crate) const DRAFT: &str = "new-issue-ai-draft";
/// Probe of the "Ask for changes" field.
pub(crate) const ASK: &str = "new-issue-ai-ask";
/// Probe of the inline error under the description.
pub(crate) const ERROR: &str = "new-issue-ai-error";
/// Id and probe of the "Try again" button.
pub(crate) const RETRY: &str = "new-issue-ai-retry";
/// Id and probe of the "Undo" button.
pub(crate) const UNDO: &str = "new-issue-ai-undo";
/// Id and probe of the "Keep" button.
pub(crate) const KEEP: &str = "new-issue-ai-keep";

/// The width the draft's Markdown is laid out at: the dialog's content inside the box's
/// border and padding and the draft block's own rule and indent.
const DRAFT_WIDTH: f32 = ui::theme::DIALOG_WIDTH - 2. * SPACE_5 - 2. - 2. * SPACE_3 - 2. - SPACE_3;

/// State of "Write with AI" in one dialog.
pub(super) struct Ai {
    /// The draft is shown instead of the text area.
    pub reviewing: bool,
    /// A draft is being written.
    pub writing: bool,
    /// The draft under review, Markdown.
    pub draft: String,
    /// The text area's text when the first draft was asked for: what Undo puts back.
    pub note: String,
    /// What went wrong, shown under the description.
    pub error: Option<SharedString>,
    /// "From your note and 2 images".
    from: SharedString,
    /// Counts resets, so an answer that arrives after one is dropped.
    epoch: u64,
    markdown: Entity<Markdown>,
    pub ask: Entity<TextInput>,
    /// The form's own focus: Cmd-Z and Cmd-J reach it while the text area is not drawn.
    pub focus: FocusHandle,
    _sub: Subscription,
}

impl Ai {
    pub(super) fn new(window: &mut Window, cx: &mut Context<NewIssueForm>) -> Self {
        let markdown = cx.new(|cx| Markdown::new("", cx).color(TEXT_SECONDARY).on_link(|url, _, cx| cx.open_url(url)));
        let ask = cx.new(|cx| TextInput::new(cx).placeholder("Ask for changes: shorter, add logs…"));
        let sub = cx.subscribe_in(&ask, window, |this: &mut NewIssueForm, _, event: &TextInputEvent, window, cx| {
            if matches!(event, TextInputEvent::Submit) {
                this.ask_for_changes(window, cx);
            }
        });
        Self {
            reviewing: false,
            writing: false,
            draft: String::new(),
            note: String::new(),
            error: None,
            from: SharedString::default(),
            epoch: 0,
            markdown,
            ask,
            focus: cx.focus_handle(),
            _sub: sub,
        }
    }

    /// Back to a plain description: "Create more" starts the next issue from nothing.
    pub(super) fn reset(&mut self, cx: &mut App) {
        self.reviewing = false;
        self.writing = false;
        self.draft.clear();
        self.note.clear();
        self.error = None;
        self.epoch += 1;
        self.ask.update(cx, |input, cx| input.set_text("", cx));
    }
}

/// "From your note and 2 images".
fn from_text(note_empty: bool, images: usize) -> String {
    let source = if note_empty { "the title" } else { "your note" };
    match images {
        0 => format!("From {source}"),
        n => format!("From {source} and {}", base::fmt::plural(n as u64, "image", "images")),
    }
}

impl NewIssueForm {
    /// Asks for a description, or for a new one while reviewing (Try again). Needs a title or
    /// a note; says so in place when there is neither.
    pub(crate) fn draft(&mut self, instruction: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.ai.writing {
            return;
        }
        let title = self.title.read(cx).text().trim().to_string();
        let previous = self.ai.reviewing.then(|| self.ai.draft.clone());
        let note =
            if self.ai.reviewing { self.ai.note.clone() } else { self.body.read(cx).text().trim().to_string() };
        if title.is_empty() && note.is_empty() {
            self.ai.error = Some("Write a title or a note first.".into());
            cx.notify();
            return;
        }
        let images: Vec<Attachment> = self.images.clone();
        let from = from_text(note.is_empty(), images.len());
        let asked = instruction.is_some();
        let request = DescriptionDraftRequest {
            title,
            note: note.clone(),
            images,
            repo: self.repo.clone(),
            previous,
            instruction,
        };
        self.ai.writing = true;
        self.ai.error = None;
        let epoch = self.ai.epoch;
        cx.notify();
        let task = hub::queries(cx).draft_description(request, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.ai.epoch != epoch {
                    return;
                }
                this.ai.writing = false;
                match result {
                    Ok(text) if !text.trim().is_empty() => {
                        if !this.ai.reviewing {
                            this.ai.note = note;
                        }
                        this.ai.draft = text.trim().to_string();
                        this.ai.from = from.into();
                        this.ai.reviewing = true;
                        let draft = this.ai.draft.clone();
                        this.ai.markdown.update(cx, |md, cx| md.set_text(draft, cx));
                        if asked {
                            this.ai.ask.update(cx, |input, cx| input.set_text("", cx));
                        }
                        // The text area is not drawn now: keep the keys (⌘Z, ⌘J) in the form.
                        if this.body.read(cx).is_focused(window) {
                            this.ai.focus.focus(window);
                        }
                    }
                    Ok(_) => this.ai.error = Some("The model wrote nothing. Try again.".into()),
                    Err(error) => this.ai.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Enter in "Ask for changes": a new draft from the current one and the instruction.
    pub(super) fn ask_for_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.ai.ask.read(cx).text().trim().to_string();
        if !text.is_empty() && self.ai.reviewing {
            self.draft(Some(text), window, cx);
        }
    }

    /// Undo: leaves review and puts the note back in the text area.
    pub(crate) fn undo_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ai.reviewing || self.ai.writing {
            return;
        }
        self.ai.reviewing = false;
        self.ai.error = None;
        let note = std::mem::take(&mut self.ai.note);
        if self.body.read(cx).text() != note {
            self.body.update(cx, |area, cx| area.set_text(&note, cx));
        }
        self.body.read(cx).focus(window);
        cx.notify();
    }

    /// Keep: leaves review; the draft is the text area's text now, to edit from here.
    pub(crate) fn keep_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ai.reviewing || self.ai.writing {
            return;
        }
        self.ai.reviewing = false;
        self.ai.error = None;
        let draft = std::mem::take(&mut self.ai.draft);
        self.body.update(cx, |area, cx| area.set_text(&draft, cx));
        self.body.read(cx).focus(window);
        cx.notify();
    }

    /// The description box: the text area (or the draft under review) and the images in a
    /// padded top part, a footer with the hint and "Write with AI" (or the violet bar) under it.
    pub(super) fn description_box(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let reviewing = self.ai.reviewing;
        let focused = !reviewing && self.body.read(cx).is_focused(window);
        let border = motion::fade_color(
            "new-issue-body-frame-border",
            if focused { ACCENT } else { LINE_STRONG },
            window,
            cx,
        );
        let top = if reviewing {
            let draft = div()
                .flex_none()
                .w(px(DRAFT_WIDTH + SPACE_3 + 2.))
                .border_l_2()
                .border_color(SPAWN_LINE)
                .pl(px(SPACE_3))
                .child(div().w(px(DRAFT_WIDTH)).child(self.ai.markdown.clone()))
                .named(DRAFT);
            col().w_full().child(draft)
        } else {
            col().w_full().child(self.body.clone())
        };
        let top = col()
            .w_full()
            .px(px(SPACE_3))
            .py(px(SPACE_2_5))
            .child(top)
            .children(self.thumbs(cx));
        let body = self.body.clone();
        let frame = col()
            .id("new-issue-body-frame")
            .debug_selector(|| "new-issue-body-frame".into())
            .w_full()
            .min_h(px(120.))
            .border_1()
            .border_color(border)
            .rounded(px(RADIUS_LG))
            .bg(SURFACE_RAISED)
            .text_color(TEXT)
            .type_style(BODY)
            .when(focused, |d| d.shadow(input_glow()))
            .when(!reviewing, |d| {
                d.on_mouse_down(MouseButton::Left, move |_, window, cx| body.read(cx).focus(window))
            })
            .child(top)
            .child(grow());
        let foot = if reviewing { self.ai_bar(cx) } else { self.footer(cx) };
        div().id("new-issue-body").w_full().child(frame.child(foot))
    }

    /// The footer: what can be attached on the left, "Write with AI" on the right.
    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let hint = if self.uploading > 0 { "Attaching…" } else { "Paste or drop images" };
        let label = if self.ai.writing { "Writing…" } else { "Write with AI" };
        let write = Button::new(WRITE, label)
            .ghost()
            .small()
            .icon(IconName::Sparkles)
            .kbd("⌘J")
            .disabled(self.ai.writing)
            .on_click(cx.listener(|this, _, window, cx| this.draft(None, window, cx)))
            .probe(WRITE);
        row()
            .flex_none()
            .h(px(40.))
            .pl(px(SPACE_3))
            .pr(px(SPACE_1_5))
            .gap(px(SPACE_2))
            .border_t_1()
            .border_color(LINE)
            .child(grow().type_style(LABEL).text_color(TEXT_MUTED).child(hint))
            .child(
                div()
                    .id("new-issue-ai-tip")
                    .flex_none()
                    .tooltip(Tooltip::text("Turn your notes, the title and images into a full description"))
                    .child(write),
            )
            .into_any_element()
    }

    /// The violet bar under a draft under review.
    fn ai_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.ai.writing;
        let label = if busy { "Writing…".into() } else { self.ai.from.clone() };
        let ask_input = self.ai.ask.clone();
        let ask = row()
            .flex_1()
            .min_w_0()
            .h(px(CONTROL_HEIGHT_SM))
            .pl(px(SPACE_2))
            .pr(px(SPACE_1_5))
            .gap(px(SPACE_2))
            .border_1()
            .border_color(SPAWN_LINE)
            .rounded(px(RADIUS_MD))
            .bg(SURFACE_RAISED)
            .text_color(TEXT)
            .type_style(LABEL)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| ask_input.read(cx).focus(window))
            .child(grow().child(self.ai.ask.clone()))
            .child(Kbd::new("↵"))
            .named(ASK);
        let retry = IconButton::new(RETRY, IconName::Replay)
            .small()
            .tooltip("Try again")
            .disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| this.draft(None, window, cx)))
            .probe(RETRY);
        let undo = Button::new(UNDO, "Undo")
            .ghost()
            .small()
            .kbd("⌘Z")
            .disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| this.undo_draft(window, cx)))
            .probe(UNDO);
        let keep = Button::new(KEEP, "Keep")
            .secondary()
            .small()
            .disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| this.keep_draft(window, cx)))
            .probe(KEEP);
        row()
            .flex_none()
            .min_h(px(44.))
            .pl(px(SPACE_3))
            .pr(px(SPACE_1_5))
            .gap(px(SPACE_1_5))
            .border_t_1()
            .border_color(SPAWN_LINE)
            .bg(SPAWN_SOFT)
            .rounded_b(px(RADIUS_LG - 1.))
            .child(div().flex().flex_none().child(icon(IconName::Sparkles, 14., SPAWN_TEXT)))
            .child(div().flex_none().whitespace_nowrap().type_style(LABEL).text_color(SPAWN_TEXT).child(label))
            .child(ask)
            .child(retry)
            .child(
                div()
                    .id("new-issue-ai-undo-tip")
                    .flex_none()
                    .tooltip(Tooltip::text("Put your note back"))
                    .child(undo),
            )
            .child(keep)
            .into_any_element()
    }
}

/// What the tests look at.
#[cfg(test)]
impl NewIssueForm {
    pub(crate) fn ai_reviewing(&self) -> bool {
        self.ai.reviewing
    }

    pub(crate) fn ai_draft(&self) -> &str {
        &self.ai.draft
    }

    pub(crate) fn ai_error(&self) -> Option<&str> {
        self.ai.error.as_ref().map(|e| e.as_ref())
    }

    pub(crate) fn ai_ask_text(&self, cx: &App) -> String {
        self.ai.ask.read(cx).text().to_string()
    }

    pub(crate) fn ai_set_writing(&mut self, writing: bool) {
        self.ai.writing = writing;
    }
}
