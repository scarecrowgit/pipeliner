//! [`Composer`]: the message box of the chat screen and of the issue panel. A [`TextArea`]
//! that grows to a maximum height, a row of attachment chips above it, `@` and `/`
//! completion menus at the caret, and a row of controls under it ending in Send (or Stop
//! while a turn runs).
//!
//! The composer knows nothing about agents, files or commands: the screen answers
//! [`ComposerEvent::TriggerChanged`] with [`Composer::set_completions`] and reads the
//! accepted [`Mention`]s back from [`ComposerEvent::Submit`].
//!
//! ```ignore
//! let composer = cx.new(|cx| Composer::chat(cx).placeholder("Message Build…"));
//! cx.subscribe(&composer, |this, composer, ev: &ComposerEvent, cx| match ev {
//!     ComposerEvent::TriggerChanged(Some(t)) => {
//!         let items = this.lookup(t);
//!         composer.update(cx, |c, cx| c.set_completions(items, None, cx));
//!     }
//!     ComposerEvent::Submit { text, attachments, mentions } => { .. }
//!     _ => {}
//! }).detach();
//! // render, with the pickers of the screen in the slots:
//! composer_view(&self.composer).leading(agent_picker).leading(model_picker)
//! ```
//!
//! Keys: Enter sends in the chat composer and Cmd/Ctrl+Enter in the issue composer
//! (Shift+Enter is always a newline). While a menu is open Up and Down move, Enter and Tab
//! insert, Escape closes it. Escape while busy asks to stop. Backspace into an accepted
//! mention deletes it whole; Backspace in an empty composer removes the last attachment.
//! Pasted images become attachments; dropped files are handed to the screen.

mod chips;
mod menu;
pub(crate) mod spans;
mod types;
mod view;

use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, Pixels, Render, ScrollHandle, SharedString,
    Subscription, Window,
};
use gpui::AppContext as _;
use ui::overlay::{Anchor, Placement, overlays};

use crate::text::{StyledRange, Token};
use crate::{SubmitOn, TextArea, TextAreaEvent};
use ui::theme::{ACCENT_SOFT, ACCENT_TEXT};

pub use spans::AttachmentLimits;
pub use types::{
    Attachment, AttachmentContent, AttachmentId, Completion, ComposerEvent, Draft, Mention,
    Trigger, TriggerKind,
};
pub use view::{ComposerView, composer_view};

/// The two composers of the mockups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerKind {
    /// The chat composer: Enter sends, a Send button with an arrow, attach before Send,
    /// `@` and `/` completions.
    Chat,
    /// The issue activity composer: Cmd/Ctrl+Enter sends, a Comment button, attach first
    /// with "Paste or drop images", `@` completions only.
    Issue,
}

/// The message box. See the module docs.
pub struct Composer {
    kind: ComposerKind,
    name: SharedString,
    area: Entity<TextArea>,
    /// The area's text as of the last sync, for moving mention spans along edits.
    text: String,
    attachments: Vec<Attachment>,
    next_id: u64,
    pasted: u64,
    limits: AttachmentLimits,
    mentions: Vec<Mention>,
    triggers: Vec<TriggerKind>,
    trigger: Option<Trigger>,
    /// Start of a trigger the user closed with Escape: it stays closed while it is typed.
    dismissed: Option<usize>,
    completions: Vec<Completion>,
    total: Option<usize>,
    selected: usize,
    menu_scroll: ScrollHandle,
    menu_key: ElementId,
    /// Where the caret was when the trigger was typed: the menu opens there.
    menu_anchor: Option<Bounds<Pixels>>,
    /// The menu's popover is up in the overlay host.
    menu_shown: bool,
    placement: Placement,
    menu_width: f32,
    busy: bool,
    disabled: Option<SharedString>,
    placeholder: SharedString,
    placeholder_hint: Option<SharedString>,
    hint: Option<SharedString>,
    send_label: SharedString,
    attach_button: bool,
    clear_on_submit: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    /// The chat composer.
    pub fn chat(cx: &mut Context<Self>) -> Self {
        Self::new(ComposerKind::Chat, cx)
    }

    /// The issue activity composer.
    pub fn issue(cx: &mut Context<Self>) -> Self {
        Self::new(ComposerKind::Issue, cx)
    }

    /// A composer of `kind` with the mockups' wording and limits.
    pub fn new(kind: ComposerKind, cx: &mut Context<Self>) -> Self {
        let (submit_on, max_rows) = match kind {
            ComposerKind::Chat => (SubmitOn::Enter, 12),
            ComposerKind::Issue => (SubmitOn::CmdEnter, 10),
        };
        let area = cx.new(|cx| TextArea::new(cx).submit_on(submit_on).rows(1, max_rows));
        let subs = vec![
            cx.subscribe(&area, Self::on_area_event),
            cx.observe(&area, |this, _, cx| this.sync(cx)),
        ];
        let (placeholder, placeholder_hint, triggers, send_label, placement, width) = match kind {
            ComposerKind::Chat => (
                "Message…",
                Some("@ files · / commands"),
                vec![TriggerKind::Mention, TriggerKind::Command],
                "Send",
                Placement::above(),
                360.,
            ),
            ComposerKind::Issue => (
                "Comment · @mention an agent or pipeline to wake it up",
                None,
                vec![TriggerKind::Mention],
                "Comment",
                Placement::below(),
                420.,
            ),
        };
        Self {
            kind,
            name: "composer".into(),
            area,
            text: String::new(),
            attachments: Vec::new(),
            next_id: 0,
            pasted: 0,
            limits: AttachmentLimits::default(),
            mentions: Vec::new(),
            triggers,
            trigger: None,
            dismissed: None,
            completions: Vec::new(),
            total: None,
            selected: 0,
            menu_scroll: ScrollHandle::new(),
            menu_key: ElementId::NamedInteger("composer-menu".into(), cx.entity_id().as_u64()),
            menu_anchor: None,
            menu_shown: false,
            placement,
            menu_width: width,
            busy: false,
            disabled: None,
            placeholder: placeholder.into(),
            placeholder_hint: placeholder_hint.map(Into::into),
            hint: None,
            send_label: send_label.into(),
            attach_button: true,
            clear_on_submit: true,
            _subs: subs,
        }
    }

    /// Builder: the name element ids and dev-driver probes start with (`chat` gives
    /// `chat-send`, `chat-text`, `chat-chip-0`, `chat-menu-0`). Unique per window.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
        self
    }

    /// Builder: the muted text shown while empty (`Message Build…`).
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Builder: the dimmer text after the placeholder (`@ files · / commands`), or none.
    pub fn placeholder_hint(mut self, text: Option<SharedString>) -> Self {
        self.placeholder_hint = text;
        self
    }

    /// Builder: which trigger characters open a completion menu.
    pub fn triggers(mut self, kinds: &[TriggerKind]) -> Self {
        self.triggers = kinds.to_vec();
        self
    }

    /// Builder: how many attachments and how large.
    pub fn limits(mut self, limits: AttachmentLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Builder: the send button's label (`Send`, `Comment`).
    pub fn send_label(mut self, label: impl Into<SharedString>) -> Self {
        self.send_label = label.into();
        self
    }

    /// Builder: show the composer's own attach button (on by default). Turn it off when the
    /// screen puts its own in a slot.
    pub fn attach_button(mut self, on: bool) -> Self {
        self.attach_button = on;
        self
    }

    /// Builder: where the completion menu opens from the caret (above in the chat, below in
    /// the issue panel) and how wide it is, px.
    pub fn menu_placement(mut self, placement: Placement, width: f32) -> Self {
        self.placement = placement;
        self.menu_width = width;
        self
    }

    /// Builder: the text area grows from `min` to `max` lines, then scrolls (default 1 to 12
    /// in the chat, 1 to 10 in the issue panel).
    pub fn rows(self, min: usize, max: usize, cx: &mut Context<Self>) -> Self {
        self.area.update(cx, |a, cx| a.set_rows(min, max, cx));
        self
    }

    /// Change how many lines the text area grows to (see [`rows`](Self::rows)).
    pub fn set_rows(&mut self, min: usize, max: usize, cx: &mut Context<Self>) {
        self.area.update(cx, |a, cx| a.set_rows(min, max, cx));
    }

    /// Builder: keep text and attachments after a submit (the screen clears them).
    pub fn keep_on_submit(mut self) -> Self {
        self.clear_on_submit = false;
        self
    }

    /// Which composer this is.
    pub fn kind(&self) -> ComposerKind {
        self.kind
    }

    /// The text as typed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The attachments, in chip order.
    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    /// The accepted mentions and commands, in text order.
    pub fn mentions(&self) -> &[Mention] {
        &self.mentions
    }

    /// The trigger being typed at the caret, if a menu could be open.
    pub fn trigger(&self) -> Option<&Trigger> {
        self.trigger.as_ref()
    }

    /// The completion menu is open (a trigger is typed and the screen supplied items).
    pub fn menu_open(&self) -> bool {
        self.trigger.is_some() && !self.completions.is_empty() && self.disabled.is_none()
    }

    /// The highlighted row of the open menu.
    pub fn selected(&self) -> Option<usize> {
        self.menu_open().then_some(self.selected)
    }

    /// The text area inside, for focus checks and tests.
    pub fn text_area(&self) -> &Entity<TextArea> {
        &self.area
    }

    /// The draft: text, attachments and mentions.
    pub fn draft(&self) -> Draft {
        Draft {
            text: self.text.clone(),
            attachments: self.attachments.clone(),
            mentions: self.mentions.clone(),
        }
    }

    /// A message would go out now: not busy or disabled, nothing uploading, and some text
    /// that is not just whitespace or at least one attachment.
    pub fn can_send(&self) -> bool {
        self.disabled.is_none()
            && !self.busy
            && self.attachments.iter().all(|a| a.progress.is_none())
            && (!self.text.trim().is_empty() || !self.attachments.is_empty())
    }

    /// A turn is running: the send button is Stop.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Why the composer is disabled, if it is.
    pub fn disabled_reason(&self) -> Option<&SharedString> {
        self.disabled.as_ref()
    }

    /// Replace the text (caret at the end). Mentions are dropped. No `DraftChanged`.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.area.update(cx, |a, cx| a.set_text(text, cx));
        self.text = text.to_string();
        self.mentions.clear();
        self.sync(cx);
        cx.notify();
    }

    /// Insert `text` at the caret as if typed (emits `DraftChanged`).
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.area.update(cx, |a, cx| a.insert_text(text, cx));
        self.sync(cx);
    }

    /// Empty the text, the attachments and the mentions. No `DraftChanged`.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.attachments.clear();
        self.set_text("", cx);
    }

    /// Put a saved draft back. Mentions whose span no longer reads their text are dropped.
    /// No `DraftChanged`.
    pub fn restore(&mut self, draft: Draft, cx: &mut Context<Self>) {
        self.set_text(&draft.text, cx);
        self.next_id = draft.attachments.iter().map(|a| a.id.0).max().unwrap_or(0).max(self.next_id);
        self.attachments = draft.attachments;
        let mut mentions = draft.mentions;
        mentions.retain(|m| self.text.get(m.range.clone()) == Some(m.text.as_ref()));
        mentions.sort_by_key(|m| m.range.start);
        self.mentions = mentions;
        self.sync(cx);
        cx.notify();
    }

    /// Give the text area keyboard focus (no-op while disabled).
    pub fn focus(&self, window: &mut Window, cx: &App) {
        self.area.read(cx).focus(window);
    }

    /// The text area has keyboard focus.
    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.area.read(cx).is_focused(window)
    }

    /// Show Stop instead of Send while `busy`; sending waits until it is cleared.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.busy = busy;
        cx.notify();
    }

    /// Disable the composer with a reason line (`Coming soon`, `Read-only session`), or
    /// enable it with `None`. A disabled composer takes no input and sends nothing.
    pub fn set_disabled(&mut self, reason: Option<SharedString>, cx: &mut Context<Self>) {
        let off = reason.is_some();
        self.disabled = reason;
        self.area.update(cx, |a, cx| a.set_disabled(off, cx));
        if off {
            self.set_trigger(None, cx);
        }
        cx.notify();
    }

    /// A muted hint before the send button (`Mentions wake agents when you post`).
    pub fn set_hint(&mut self, hint: Option<SharedString>, cx: &mut Context<Self>) {
        self.hint = hint;
        cx.notify();
    }

    /// Change the placeholder (`Message Plan…` after switching agents).
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = text.into();
        cx.notify();
    }

    /// Change which key sends.
    pub fn set_submit_on(&mut self, mode: SubmitOn, cx: &mut Context<Self>) {
        self.area.update(cx, |a, cx| a.set_submit_on(mode, cx));
    }

    /// The rows for the current trigger: `items` match its query, `total` is how many there
    /// are in all (for "Matching “re” · 2 of 8"). The first row is highlighted. Ignored when
    /// no trigger is being typed.
    pub fn set_completions(
        &mut self,
        items: Vec<Completion>,
        total: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.trigger.is_none() {
            return;
        }
        self.completions = items;
        self.total = total;
        self.selected = 0;
        self.menu_scroll.scroll_to_item(0);
        self.apply_intercept(cx);
        cx.notify();
    }

    /// Add an attachment (from the file picker, a drop, or the screen). Refused when the
    /// composer is full or the file is too large; the error says why. Emits `DraftChanged`.
    pub fn add_attachment(
        &mut self,
        mut attachment: Attachment,
        cx: &mut Context<Self>,
    ) -> Result<AttachmentId, SharedString> {
        if let Some(why) = spans::refusal(&attachment, self.attachments.len(), self.limits) {
            return Err(why.into());
        }
        self.next_id += 1;
        attachment.id = AttachmentId(self.next_id);
        let id = attachment.id;
        self.attachments.push(attachment);
        cx.emit(ComposerEvent::DraftChanged);
        cx.notify();
        Ok(id)
    }

    /// Remove an attachment. Emits `DraftChanged` when it was there.
    pub fn remove_attachment(&mut self, id: AttachmentId, cx: &mut Context<Self>) -> bool {
        let before = self.attachments.len();
        self.attachments.retain(|a| a.id != id);
        let removed = self.attachments.len() != before;
        if removed {
            cx.emit(ComposerEvent::DraftChanged);
            cx.notify();
        }
        removed
    }

    /// Set an attachment's upload progress (0 to 1), or `None` once it is uploaded.
    pub fn set_attachment_progress(
        &mut self,
        id: AttachmentId,
        progress: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        if let Some(a) = self.attachments.iter_mut().find(|a| a.id == id) {
            a.progress = progress.map(|p| p.clamp(0., 1.));
            cx.notify();
        }
    }

    /// Send what is typed, as the send key does. Returns whether a `Submit` went out.
    pub fn submit(&mut self, cx: &mut Context<Self>) -> bool {
        self.sync(cx);
        if !self.can_send() {
            return false;
        }
        let text = self.text.trim_end().to_string();
        let mentions =
            self.mentions.iter().filter(|m| m.range.end <= text.len()).cloned().collect();
        let attachments = self.attachments.clone();
        cx.emit(ComposerEvent::Submit { text, attachments, mentions });
        if self.clear_on_submit {
            self.clear(cx);
            cx.emit(ComposerEvent::DraftChanged);
        }
        true
    }

    /// Choose row `ix` of the open menu: it replaces the typed trigger and query, followed by
    /// a space, and is kept as a [`Mention`].
    pub fn accept(&mut self, ix: usize, cx: &mut Context<Self>) -> bool {
        let (Some(t), Some(item)) = (self.trigger.clone(), self.completions.get(ix).cloned()) else {
            return false;
        };
        if !self.menu_open() {
            return false;
        }
        let token = Token { trigger: t.kind.char(), start: t.start, text: t.query.clone() };
        let insert = format!("{} ", item.insert);
        let done = self.area.update(cx, |a, cx| a.replace_token(&token, &insert, cx));
        if !done {
            return false;
        }
        // Move the old spans along now, so the new one is not taken for part of the edit.
        self.sync_text(cx);
        let start = self.area.read(cx).cursor().saturating_sub(insert.len());
        let range = start..start + item.insert.len();
        if self.text.get(range.clone()) == Some(item.insert.as_ref()) {
            self.mentions.push(Mention { kind: t.kind, id: item.id, text: item.insert, range });
            self.mentions.sort_by_key(|m| m.range.start);
        }
        self.sync(cx);
        true
    }

    // ---------------------------------------------------------------- internals

    fn on_area_event(&mut self, _: Entity<TextArea>, ev: &TextAreaEvent, cx: &mut Context<Self>) {
        match ev {
            TextAreaEvent::Changed => self.sync(cx),
            TextAreaEvent::Submit => {
                self.submit(cx);
            }
            TextAreaEvent::Cancel => {
                if self.menu_open() {
                    self.dismiss(cx);
                } else if self.busy {
                    cx.emit(ComposerEvent::Stop);
                }
            }
            TextAreaEvent::Up => self.move_selection(false, cx),
            TextAreaEvent::Down => self.move_selection(true, cx),
            TextAreaEvent::Tab | TextAreaEvent::Confirm => {
                let ix = self.selected;
                self.accept(ix, cx);
            }
            TextAreaEvent::PasteImage(item) => self.paste_images(item, cx),
            TextAreaEvent::Focus | TextAreaEvent::Blur => cx.notify(),
        }
    }

    /// Catch up with the area's text: move mention spans, report the draft change.
    fn sync_text(&mut self, cx: &mut Context<Self>) {
        let area = self.area.read(cx);
        if area.text() == self.text {
            return;
        }
        let new = area.text().to_string();
        spans::reconcile(&self.text, &new, &mut self.mentions);
        self.text = new;
        cx.emit(ComposerEvent::DraftChanged);
        cx.notify();
    }

    /// Draw the accepted mentions in the text as the mockups' `.mention`: `accent-text` on
    /// `accent-soft`.
    fn style_mentions(&mut self, cx: &mut Context<Self>) {
        let styles: Vec<StyledRange> = self
            .mentions
            .iter()
            .map(|m| StyledRange::new(m.range.clone()).color(ACCENT_TEXT).background(ACCENT_SOFT))
            .collect();
        if self.area.read(cx).styles() != styles.as_slice() {
            self.area.update(cx, |a, cx| a.set_styles(styles, cx));
        }
    }

    /// Catch up with the area after any change or caret move: text, then the trigger.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.sync_text(cx);
        self.style_mentions(cx);
        let chars: Vec<char> = self.triggers.iter().map(|k| k.char()).collect();
        let token = self.area.read(cx).token_before_cursor(&chars);
        let trigger = token
            .filter(|_| self.disabled.is_none())
            .filter(|t| !self.mentions.iter().any(|m| m.range.start == t.start))
            .and_then(|t| {
                Some(Trigger { kind: TriggerKind::from_char(t.trigger)?, query: t.text, start: t.start })
            });
        if trigger.as_ref().map(|t| t.start) != self.dismissed {
            self.dismissed = None;
        }
        let trigger = trigger.filter(|t| Some(t.start) != self.dismissed);
        self.set_trigger(trigger, cx);
    }

    fn set_trigger(&mut self, trigger: Option<Trigger>, cx: &mut Context<Self>) {
        if trigger == self.trigger {
            return;
        }
        let key = |t: &Option<Trigger>| t.as_ref().map(|t| (t.kind, t.start));
        if trigger.is_none() || key(&trigger) != key(&self.trigger) {
            self.completions.clear();
            self.total = None;
            self.selected = 0;
            self.menu_anchor = trigger.as_ref().and_then(|_| self.area.read(cx).caret_bounds());
        }
        self.trigger = trigger.clone();
        cx.emit(ComposerEvent::TriggerChanged(trigger));
        self.apply_intercept(cx);
        cx.notify();
    }

    /// Escape on an open menu: close it and keep it closed while this trigger is typed.
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.dismissed = self.trigger.as_ref().map(|t| t.start);
        self.set_trigger(None, cx);
    }

    fn apply_intercept(&mut self, cx: &mut Context<Self>) {
        let on = self.menu_open();
        self.area.update(cx, |a, _| a.intercept_arrows(on));
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.completions.len();
        if !self.menu_open() || n == 0 {
            return;
        }
        self.selected = if down { (self.selected + 1) % n } else { (self.selected + n - 1) % n };
        self.menu_scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    /// The pointer moved over row `ix`.
    pub(crate) fn hover_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.selected != ix && ix < self.completions.len() {
            self.selected = ix;
            cx.notify();
        }
    }

    fn paste_images(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) {
        for entry in item.entries() {
            let ClipboardEntry::Image(image) = entry else { continue };
            self.pasted += 1;
            let name = spans::pasted_name(self.pasted, image.format);
            let attachment = Attachment::image(name, Arc::new(image.clone()));
            if let Err(why) = self.add_attachment(attachment, cx) {
                cx.emit(ComposerEvent::AttachmentRejected(why));
            }
        }
    }

    /// The user removed a chip (its × or Backspace in an empty composer).
    pub(crate) fn remove_by_user(&mut self, id: AttachmentId, cx: &mut Context<Self>) {
        if self.remove_attachment(id, cx) {
            cx.emit(ComposerEvent::AttachmentRemoved(id));
        }
    }

    /// Backspace or Delete is about to cut into a mention: select all of it, so the key
    /// deletes it whole.
    pub(crate) fn select_mention_for_delete(&mut self, left: bool, cx: &mut Context<Self>) {
        let area = self.area.read(cx);
        if !area.selection().is_empty() {
            return;
        }
        if let Some(range) = spans::mention_at(&self.mentions, area.cursor(), left) {
            self.area.update(cx, |a, cx| a.select_range(range, cx));
        }
    }

    /// Backspace that the area did not use (caret at the start): drop the last attachment
    /// when there is no text.
    pub(crate) fn backspace_at_start(&mut self, cx: &mut Context<Self>) {
        if self.text.is_empty()
            && self.disabled.is_none()
            && let Some(id) = self.attachments.last().map(|a| a.id)
        {
            self.remove_by_user(id, cx);
        }
    }

    pub(crate) fn emit(&mut self, ev: ComposerEvent, cx: &mut Context<Self>) {
        cx.emit(ev);
    }

    /// The overlay host does not match the menu state: open or close the popover there.
    pub(crate) fn overlay_out_of_step(&self, window: &Window, cx: &App) -> bool {
        let o = overlays(window, cx);
        if !o.is_mounted(cx) {
            return false;
        }
        let shown_now = self.menu_shown && o.is_open(self.menu_key.clone(), cx);
        self.menu_open() != self.menu_shown || shown_now != self.menu_shown
    }

    /// Bring the overlay host in step with the menu state (run outside rendering).
    pub(crate) fn sync_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let o = overlays(window, cx);
        if self.menu_shown && !o.is_open(self.menu_key.clone(), cx) {
            // Closed from outside (a click elsewhere).
            self.menu_shown = false;
            if self.menu_open() {
                self.dismiss(cx);
            }
            return;
        }
        let want = self.menu_open();
        if want && !self.menu_shown {
            let mut anchor = Anchor::of(self.menu_key.clone(), window, cx);
            if let Some(b) = self.menu_anchor.or_else(|| self.area.read(cx).caret_bounds()) {
                anchor.bounds = b;
            }
            let focused = self.is_focused(window, cx);
            let me = cx.weak_entity();
            let opened = o.open_popover(
                anchor,
                self.placement,
                move |window, cx| menu::build(&me, window, cx),
                window,
                cx,
            );
            self.menu_shown = opened.is_some();
            // The host moves focus into a new layer; typing stays in the text.
            if focused {
                self.focus(window, cx);
            }
        } else if !want && self.menu_shown {
            self.menu_shown = false;
            o.close(self.menu_key.clone(), window, cx);
        }
    }
}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.area.focus_handle(cx)
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        composer_view(&cx.entity())
    }
}
