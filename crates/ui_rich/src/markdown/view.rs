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
use ui::theme::{BODY, SPACE_3, TEXT, TypeStyle};

use super::cache::parse_cached;
use super::element::{Shared, Tracker};
use super::group::SelectionGroup;
use super::parse::Document;
use super::render::Renderer;
use super::rich::{Unit, UnitKind, units, verbatim_unit};

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

/// Selectable, copyable text of one message, banner or tool output.
///
/// [`Markdown::new`] renders Markdown (agent or user text): paragraphs, headings, lists,
/// quotes, code blocks with copy buttons, tables, links and `@mentions`.
/// [`Markdown::verbatim`] shows any other text exactly as given (errors, tool inputs and
/// outputs, log lines): nothing is Markdown, every `\n` breaks the line, spaces are kept,
/// long lines wrap (between graphemes inside a word too long for the line).
///
/// Either way the text can be selected with the mouse and copied with Cmd/Ctrl+C, or
/// selected whole with Cmd/Ctrl+A (after [`bind_keys`]); a press anywhere else in the
/// window drops the selection, so only one view shows one. [`type_style`](Self::type_style)
/// and [`color`](Self::color) set the look of body text in both modes.
///
/// # Placement
///
/// The view takes the full width of its parent and wraps to it, so the parent needs a
/// width the layout can resolve before measuring text: a fixed width, a stretched column
/// child, or a `flex_1().min_w_0()` child of a row. Never put it straight into a
/// shrink-to-fit or content-sized row child: it is then measured at one-line width.
///
/// ```ignore
/// let md = cx.new(|cx| Markdown::new(text, cx).on_link(|url, _, cx| cx.open_url(url)));
/// md.update(cx, |md, cx| md.set_text(streamed_so_far, cx));
///
/// // A failure banner: icon | text | button.
/// let err = cx.new(|cx| Markdown::verbatim(message, cx).type_style(LABEL).color(DANGER));
/// row().items_start().child(icon).child(div().flex_1().min_w_0().child(err)).child(retry)
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
    style: TypeStyle,
    verbatim: bool,
    copied: Option<usize>,
    copied_reset: Option<Task<()>>,
    group: Option<(SelectionGroup, usize)>,
}

impl Markdown {
    /// A view showing `text` as Markdown.
    pub fn new(text: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let text = text.into();
        let doc = parse_cached(&text, None);
        let units = Rc::new(units(&doc, &[]));
        Self::with(text, doc, units, false, cx)
    }

    /// A view showing `text` verbatim: no Markdown syntax, every newline a line break,
    /// leading spaces kept, tabs shown as four spaces (copied as tabs). Mentions and links
    /// do not apply. With a mono style (`MONO`, `MONO_SM`) it looks like a code block's text
    /// without the frame; the frame, padding and any line cap are the caller's.
    pub fn verbatim(text: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let text = text.into();
        let units = Rc::new(vec![verbatim_unit(&text)]);
        Self::with(text, Arc::default(), units, true, cx)
    }

    fn with(
        text: SharedString,
        doc: Arc<Document>,
        units: Rc<Vec<Unit>>,
        verbatim: bool,
        cx: &mut Context<Self>,
    ) -> Self {
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
            style: BODY,
            verbatim,
            copied: None,
            copied_reset: None,
            group: None,
        }
    }

    /// Share one selection with the other views of `group`, as member `key` (keys order the
    /// members: a drag selects every member between its ends, a copy joins them in key
    /// order). See [`SelectionGroup`].
    pub fn group(mut self, group: &SelectionGroup, key: usize) -> Self {
        self.group = Some((group.clone(), key));
        self
    }

    /// Join `group` as member `key`, or leave any group with `None`; the view's own selection
    /// is dropped either way.
    pub fn set_group(&mut self, group: Option<(&SelectionGroup, usize)>, cx: &mut Context<Self>) {
        let next = group.map(|(g, k)| (g.clone(), k));
        let same = match (&self.group, &next) {
            (Some((a, ka)), Some((b, kb))) => a.ptr_eq(b) && ka == kb,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.group = next;
        self.shared.borrow_mut().sel.clear();
        cx.notify();
    }

    /// The group the view shares its selection with, if any.
    pub fn selection_group(&self) -> Option<&SelectionGroup> {
        self.group.as_ref().map(|(g, _)| g)
    }

    /// The type style of body text (paragraphs, list items, table cells; all of a verbatim
    /// text): `BODY` by default, `LABEL` or `CAPTION` for small text, `MONO`/`MONO_SM` for
    /// tool output. Headings keep their own sizes.
    pub fn type_style(mut self, style: TypeStyle) -> Self {
        self.style = style;
        self
    }

    /// Change the type style of body text.
    pub fn set_type_style(&mut self, style: TypeStyle, cx: &mut Context<Self>) {
        if self.style != style {
            self.style = style;
            cx.notify();
        }
    }

    /// Change the body text colour.
    pub fn set_color(&mut self, color: Hsla, cx: &mut Context<Self>) {
        if self.color != color {
            self.color = color;
            cx.notify();
        }
    }

    /// The view shows its text verbatim rather than as Markdown.
    pub fn is_verbatim(&self) -> bool {
        self.verbatim
    }

    /// Names shown in the mention look when written as `@name` in plain text (Markdown mode
    /// only).
    pub fn mentions(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.mentions = names.into_iter().map(Into::into).collect();
        if self.verbatim {
            return self;
        }
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

    /// The body text colour: `TEXT` (the default, user messages), `TEXT_SECONDARY` (agent
    /// prose in the transcript), `DANGER` (a failure).
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = color;
        self
    }

    /// Replace the text. A text that extends the current one (a streaming message) only
    /// re-parses its last block, or in verbatim mode only adds the new part; the selection
    /// is kept.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        if text == self.text {
            return;
        }
        if self.verbatim {
            let unit = match (text.strip_prefix(self.text.as_ref()), self.units.first()) {
                (Some(more), Some(old)) => {
                    let mut unit = old.clone();
                    Rc::make_mut(&mut unit.text).extend_verbatim(more);
                    unit
                }
                _ => verbatim_unit(&text),
            };
            self.text = text;
            self.units = Rc::new(vec![unit]);
            cx.notify();
            return;
        }
        self.doc = parse_cached(&text, Some((&self.text, &self.doc)));
        self.text = text;
        self.units = Rc::new(units(&self.doc, &self.mentions));
        cx.notify();
    }

    /// Replace the mention names (Markdown mode only).
    pub fn set_mentions(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        self.mentions = names;
        if self.verbatim {
            return;
        }
        self.units = Rc::new(units(&self.doc, &self.mentions));
        cx.notify();
    }

    /// The source text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The parsed document (empty in verbatim mode).
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The selected text of this view, if anything of it is selected. In a group, only this
    /// view's part; [`SelectionGroup::selected_text`] is what a copy gives.
    pub fn selected_text(&self) -> Option<String> {
        self.shared.borrow().sel.text(&self.units)
    }

    /// Select all text (in a group: everything the group knows).
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some((group, key)) = self.group.clone() {
            // This view is borrowed: join first so the group reaches it, then select.
            let id = cx.entity_id();
            group.join(key, &self.shared, &self.units, id, &self.focus);
            group.select_all(cx);
            return;
        }
        self.shared.borrow_mut().sel.select_all(&self.units);
        cx.notify();
    }

    /// Drop the selection.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.shared.borrow_mut().sel.clear();
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = match &self.group {
            Some((group, _)) => group.selected_text(),
            None => self.selected_text(),
        };
        match text {
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
        if let Some((group, key)) = &self.group {
            group.join(*key, &self.shared, &self.units, cx.entity_id(), &self.focus);
        }
        let focused = self.focus.is_focused(window) || self.group.as_ref().is_some_and(|(g, _)| g.focused(window));
        let doc = self.doc.clone();
        let units = self.units.clone();
        let mut r = Renderer::new(
            &units,
            self.shared.clone(),
            focused,
            self.color,
            self.style,
            self.copied,
        );
        let blocks: Vec<_> = if self.verbatim {
            vec![r.verbatim()]
        } else {
            doc.blocks.iter().map(|b| r.block(b, window, cx)).collect()
        };
        let tracker = Tracker {
            shared: self.shared.clone(),
            view: cx.entity().downgrade(),
            focus: self.focus.clone(),
            on_link: self.on_link.clone(),
            group: self.group.clone(),
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
