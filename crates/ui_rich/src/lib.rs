//! Text area, markdown, diff view, composer, charts and canvas primitives.
//!
//! Text editing:
//! - [`TextArea`]: multi-line editing (composer, descriptions, comments, role editor).
//! - [`TextInput`]: single-line editing (search, titles, dialog inputs).
//! - [`text_field`] / [`text_area_field`]: either one inside `ui`'s `InputFrame`.
//! - [`Composer`]: the chat and issue message box (attachments, `@` / `/` menus, send).
//! - [`text`]: the shared core ([`text::Buffer`] is GPUI-free) and [`bind_keys`], which the
//!   app calls once at startup.
//!
//! Images:
//! - [`viewer`]: the full-window image viewer (zoom, pan, several images) and
//!   [`viewer::ImageThumb`], the clickable thumbnail that opens it.

pub mod canvas;
pub mod charts;
pub mod composer;
pub mod diff_view;
pub mod field;
pub mod markdown;
pub mod text;
pub mod text_area;
pub mod text_input;
pub mod viewer;

#[cfg(test)]
mod tests;

pub use composer::{
    Attachment, AttachmentContent, AttachmentId, AttachmentLimits, Completion, Composer,
    ComposerEvent, ComposerKind, ComposerView, Draft, Mention, Trigger, TriggerKind,
    composer_view,
};
pub use field::{TextAreaField, TextField, text_area_field, text_field};
pub use text::{Token, bind_keys};
pub use text_area::{SubmitOn, TextArea, TextAreaEvent};
pub use text_input::{TextInput, TextInputEvent};
