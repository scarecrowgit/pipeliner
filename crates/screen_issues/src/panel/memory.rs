//! What the panel remembers while the app runs: the unsent comment of each issue, with the
//! images of it already stored. `hub` has no place for drafts, so they live here, in a GPUI
//! global, and survive the panel being closed and opened again.

use std::collections::HashMap;

use gpui::{App, Global};
use hub::api::Attachment as Uploaded;
use hub::model::IssueKey;
use ui_rich::{AttachmentId, Draft};

/// A draft and the images of it that already reached the blob store.
#[derive(Clone, Debug, Default)]
pub(super) struct Saved {
    pub draft: Draft,
    pub uploaded: HashMap<AttachmentId, Uploaded>,
}

#[derive(Default)]
struct Memory {
    drafts: HashMap<IssueKey, Saved>,
}

impl Global for Memory {}

/// The saved draft of `issue`, if any.
pub(super) fn draft(issue: &IssueKey, cx: &App) -> Option<Saved> {
    cx.try_global::<Memory>().and_then(|m| m.drafts.get(issue).cloned())
}

/// Keeps `saved` as the draft of `issue`; an empty draft forgets it.
pub(super) fn save_draft(issue: &IssueKey, saved: Saved, cx: &mut App) {
    let empty = saved.draft.text.trim().is_empty() && saved.draft.attachments.is_empty();
    let memory = cx.default_global::<Memory>();
    if empty {
        memory.drafts.remove(issue);
    } else {
        memory.drafts.insert(issue.clone(), saved);
    }
}
