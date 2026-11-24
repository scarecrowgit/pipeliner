//! The plain data a [`Composer`](super::Composer) takes in and hands out: attachments,
//! mentions, triggers, completion items, events and drafts.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Image, SharedString};
use ui::assets::IconName;

/// Identifies one attachment of a composer, from
/// [`Composer::add_attachment`](super::Composer::add_attachment). Unique for the composer's
/// lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AttachmentId(pub u64);

/// What an attachment holds: image bytes in memory, or a file on disk.
#[derive(Clone, Debug, PartialEq)]
pub enum AttachmentContent {
    /// An image held in memory (pasted from the clipboard, or handed in by the screen).
    Image(Arc<Image>),
    /// An image file on disk; the chip shows it as a thumbnail.
    ImageFile(PathBuf),
    /// Any other file on disk; the chip shows its name.
    File(PathBuf),
}

/// One file or image waiting to go out with the next message.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    /// Set by the composer when the attachment is added.
    pub id: AttachmentId,
    /// File name shown on the chip (`retry-log.png`).
    pub name: SharedString,
    /// Size in bytes, checked against the composer's limit and shown on the chip.
    pub size: u64,
    /// The bytes or the path.
    pub content: AttachmentContent,
    /// Upload progress from 0 to 1 while the screen uploads it; `None` when it is ready.
    /// A message cannot be sent while anything is uploading.
    pub progress: Option<f32>,
}

impl Attachment {
    /// An in-memory image; its size is the length of its bytes.
    pub fn image(name: impl Into<SharedString>, image: Arc<Image>) -> Self {
        let size = image.bytes.len() as u64;
        Self::new(name, size, AttachmentContent::Image(image))
    }

    /// An image file on disk of `size` bytes.
    pub fn image_file(name: impl Into<SharedString>, path: PathBuf, size: u64) -> Self {
        Self::new(name, size, AttachmentContent::ImageFile(path))
    }

    /// Any other file on disk of `size` bytes.
    pub fn file(name: impl Into<SharedString>, path: PathBuf, size: u64) -> Self {
        Self::new(name, size, AttachmentContent::File(path))
    }

    fn new(name: impl Into<SharedString>, size: u64, content: AttachmentContent) -> Self {
        Self { id: AttachmentId(0), name: name.into(), size, content, progress: None }
    }

    /// Builder: start in the uploading state at `progress` (0 to 1).
    pub fn uploading(mut self, progress: f32) -> Self {
        self.progress = Some(progress.clamp(0., 1.));
        self
    }

    /// The chip shows a thumbnail.
    pub fn is_image(&self) -> bool {
        !matches!(self.content, AttachmentContent::File(_))
    }
}

/// Which completion a trigger character opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    /// `@`: files, agents, pipelines.
    Mention,
    /// `/`: commands.
    Command,
}

impl TriggerKind {
    /// The character that starts it.
    pub const fn char(self) -> char {
        match self {
            TriggerKind::Mention => '@',
            TriggerKind::Command => '/',
        }
    }

    /// The kind `c` starts, if any.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '@' => Some(TriggerKind::Mention),
            '/' => Some(TriggerKind::Command),
            _ => None,
        }
    }

    /// Heading of the completion menu while nothing follows the trigger.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            TriggerKind::Mention => "Mention",
            TriggerKind::Command => "Commands",
        }
    }
}

/// The `@word` or `/word` being typed at the caret: what the screen looks up completions for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    /// `@` or `/`.
    pub kind: TriggerKind,
    /// What follows the trigger character, up to the caret (`re` for `@re`).
    pub query: String,
    /// Byte offset of the trigger character in the text.
    pub start: usize,
}

/// One row of a completion menu, supplied by the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    /// The screen's key for the thing (an agent id, a path); comes back on the [`Mention`].
    pub id: SharedString,
    /// Row label (`Reproducer`, `src/charges/retry.ts`).
    pub label: SharedString,
    /// Muted line under the label.
    pub detail: Option<SharedString>,
    /// Icon in a tile before the label.
    pub icon: Option<IconName>,
    /// Short muted word at the right (`running`, `idle`).
    pub hint: Option<SharedString>,
    /// What replaces the typed trigger and query (`@Reproducer`). A space follows it.
    pub insert: SharedString,
}

impl Completion {
    /// A row labelled `label` that inserts `insert`.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        insert: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            icon: None,
            hint: None,
            insert: insert.into(),
        }
    }

    /// Builder: a muted line under the label.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Builder: an icon before the label.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Builder: a hint at the right.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// An accepted completion kept as a span of the text, read back on submit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// `@` or `/`.
    pub kind: TriggerKind,
    /// The [`Completion::id`] that was chosen.
    pub id: SharedString,
    /// The inserted text (`@Reproducer`).
    pub text: SharedString,
    /// Byte range of `text` in the message.
    pub range: Range<usize>,
}

/// Everything typed so far, for persisting and restoring a draft.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    /// The text as typed.
    pub text: String,
    /// The attachments, in chip order.
    pub attachments: Vec<Attachment>,
    /// The accepted mentions, in text order.
    pub mentions: Vec<Mention>,
}

/// What a [`Composer`](super::Composer) tells its screen.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposerEvent {
    /// The user sent the message (send key or button). The text has trailing whitespace
    /// trimmed; mention ranges index into it. The composer then clears itself.
    Submit {
        /// The message.
        text: String,
        /// What goes with it.
        attachments: Vec<Attachment>,
        /// The accepted mentions and commands in the text.
        mentions: Vec<Mention>,
    },
    /// The user asked to stop the running turn (the Stop button, or Escape while busy).
    Stop,
    /// A trigger appeared, its query changed, or it went away (`None`). Answer with
    /// [`Composer::set_completions`](super::Composer::set_completions).
    TriggerChanged(Option<Trigger>),
    /// The text or the attachments changed by the user's hand: persist the draft.
    DraftChanged,
    /// The attach button was pressed: open a file picker and call `add_attachment`.
    AttachRequested,
    /// Files were dropped on the composer. The screen reads their sizes off the UI thread
    /// and calls `add_attachment`.
    FilesDropped(Vec<PathBuf>),
    /// The user removed an attachment (or cancelled its upload).
    AttachmentRemoved(AttachmentId),
    /// A pasted or added attachment was refused; the reason says why and what to do.
    AttachmentRejected(SharedString),
}
