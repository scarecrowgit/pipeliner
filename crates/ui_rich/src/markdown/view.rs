//! The [`Markdown`] view: one message's text, parsed once, rendered with selection, copy,
//! link clicks and code-block copy buttons.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, ClipboardItem, Context, FocusHandle, Focusable, Hsla, IntoElement, KeyBinding, Render,
    SharedString, Task, Window, actions, prelude::*,
};
use ui::layout::col;
use ui::theme::{SPACE_3, TEXT};

use super::cache::parse_cached;
use super::element::{Shared, Tracker};
use super::parse::Document;
use super::render::Renderer;
use super::rich::{Unit, UnitKind, units};

actions!(
    markdown,
    [
        /// Copy the selected text of a message.
        Copy,
        /// Select all text of a message.
        SelectAll,
    ]
);

/// Key context of a focused [`Markdown`] view.
pub const KEY_CONTEXT: &str = "Markdown";

/// Bind Cmd/Ctrl+C and Cmd/Ctrl+A for focused [`Markdown`] views. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    let c = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("ctrl-c", Copy, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("ctrl-a", SelectAll, c),
    ]);
}

/// A callback that receives a string: the URL of a clicked link, the text of a copied code
/// block.
pub type LinkHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// How long a code block's copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_millis(1500);

/// Rendered Markdown of one message (agent or user text): paragraphs, headings, lists,
/// quotes, code blocks with copy buttons, tables, links and `@mentions`. The text can be
/// selected with the mouse across blocks and copied with Cmd/Ctrl+C (after [`bind_keys`]).
///
/// ```ignore
/// let md = cx.new(|cx| Markdown::new(text, cx).on_link(|url, _, cx| cx.open_url(url)));
/// md.update(cx, |md, cx| md.set_text(streamed_so_far, cx));
/// ```
pub struct Markdown {
    text: SharedString,
    doc: Arc<Document>,
    pub(crate) units: Rc<Vec<Unit>>,
    mentions: Vec<String>,
    pub(crate) shared: Rc<RefCell<Shared>>,
    focus: FocusHandle,
    on_link: Option<LinkHandler>,
    on_copy: Option<LinkHandler>,
    color: Hsla,
    copied: Option<usize>,
    copied_reset: Option<Task<()>>,
}

impl Markdown {
    /// A view showing `text`.
    pub fn new(text: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let text = text.into();
        let doc = parse_cached(&text, None);
        let units = Rc::new(units(&doc, &[]));
        Self {
            text,
            doc,
            units,
            mentions: Vec::new(),
            shared: Rc::default(),
            focus: cx.focus_handle(),
            on_link: None,
            on_copy: None,
            color: TEXT,
            copied: None,
            copied_reset: None,
        }
    }

    /// Names shown in the mention look when written as `@name` in plain text.
    pub fn mentions(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.mentions = names.into_iter().map(Into::into).collect();
        self.units = Rc::new(units(&self.doc, &self.mentions));
        self
    }

    /// Called with the URL when a link is clicked.
    pub fn on_link(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_link = Some(Rc::new(f));
        self
    }

    /// Called with the code after a code block's copy button put it on the clipboard.
    pub fn on_copy_code(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_copy = Some(Rc::new(f));
        self
    }

    /// The body text colour: `TEXT` (the default, user messages) or `TEXT_SECONDARY`
    /// (agent prose in the transcript).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = color;
        self
    }

    /// Replace the text. A text that extends the current one (a streaming message) only
    /// re-parses its last block; the selection is kept.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        if text == self.text {
            return;
        }
        self.doc = parse_cached(&text, Some((&self.text, &self.doc)));
        self.text = text;
        self.units = Rc::new(units(&self.doc, &self.mentions));
        cx.notify();
    }

    /// Replace the mention names.
    pub fn set_mentions(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        self.mentions = names;
        self.units = Rc::new(units(&self.doc, &self.mentions));
        cx.notify();
    }

    /// The Markdown source.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The parsed document.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The selected text as it would be copied, if anything is selected.
    pub fn selected_text(&self) -> Option<String> {
        self.shared.borrow().sel.text(&self.units)
    }

    /// Select all text.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.shared.borrow_mut().sel.select_all(&self.units);
        cx.notify();
    }

    /// Drop the selection.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.shared.borrow_mut().sel.clear();
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected_text() {
            Some(t) => cx.write_to_clipboard(ClipboardItem::new_string(t)),
            None => cx.propagate(),
        }
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }

    /// Copy the code of unit `unit` (a code block), the `nth` code block of the document.
    pub(crate) fn copy_code(
        &mut self,
        unit: usize,
        nth: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(UnitKind::Code { raw, .. }) = self.units.get(unit).map(|u| &u.kind) else {
            return;
        };
        let raw = raw.clone();
        cx.write_to_clipboard(ClipboardItem::new_string(raw.to_string()));
        if let Some(f) = self.on_copy.clone() {
            f(&raw, window, cx);
        }
        self.copied = Some(nth);
        self.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Focusable for Markdown {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Markdown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shared.borrow_mut().frame.clear();
        let doc = self.doc.clone();
        let units = self.units.clone();
        let mut r = Renderer::new(
            &units,
            self.shared.clone(),
            self.focus.is_focused(window),
            self.color,
            self.copied,
        );
        let blocks: Vec<_> = doc.blocks.iter().map(|b| r.block(b, window, cx)).collect();
        let tracker = Tracker {
            shared: self.shared.clone(),
            view: cx.entity().downgrade(),
            focus: self.focus.clone(),
            on_link: self.on_link.clone(),
        };
        col()
            .relative()
            .w_full()
            .flex_none()
            .gap(gpui::px(SPACE_3))
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all_action))
            .children(blocks)
            .child(tracker)
    }
}
