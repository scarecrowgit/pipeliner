//! What the chat screen remembers while the app runs: the unsent draft of each session and
//! whether the context panel is open. `hub` has no place for drafts, so they live here, in a
//! GPUI global, and survive the session view being dropped and built again.

use std::collections::HashMap;

use gpui::{App, Global};
use hub::api::Attachment as Uploaded;
use hub::model::SessionId;
use ui_rich::{AttachmentId, Draft};

/// A draft and the images of it that already reached the blob store.
#[derive(Clone, Debug, Default)]
pub(super) struct Saved {
    pub draft: Draft,
    pub uploaded: HashMap<AttachmentId, Uploaded>,
}

#[derive(Default)]
struct Memory {
    drafts: HashMap<SessionId, Saved>,
    panel_closed: bool,
}

impl Global for Memory {}

/// The saved draft of `session`, if any.
pub(super) fn draft(session: &SessionId, cx: &App) -> Option<Saved> {
    cx.try_global::<Memory>().and_then(|m| m.drafts.get(session).cloned())
}

/// Keeps `saved` as the draft of `session`; an empty draft forgets it.
pub(super) fn save_draft(session: &SessionId, saved: Saved, cx: &mut App) {
    let empty = saved.draft.text.trim().is_empty() && saved.draft.attachments.is_empty();
    let memory = cx.default_global::<Memory>();
    if empty {
        memory.drafts.remove(session);
    } else {
        memory.drafts.insert(session.clone(), saved);
    }
}

/// The context panel is shown (it is unless someone closed it).
pub(super) fn panel_open(cx: &App) -> bool {
    !cx.try_global::<Memory>().is_some_and(|m| m.panel_closed)
}

/// Remembers whether the context panel is shown.
pub(super) fn set_panel_open(open: bool, cx: &mut App) {
    cx.default_global::<Memory>().panel_closed = !open;
}
