//! The Inbox: items, open reviews and the "N waiting" count ([`InboxModel`]), and one opened
//! review with its variant diffs loaded on demand ([`ReviewModel`]).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use gpui::Context;

use crate::api::{Api, FileDiff, InboxId, InboxItem, Review, ReviewId, ReviewState};
use crate::load::{LoadState, Loadable, failed, fetch, query};

pub(crate) enum InboxInput {
    Item(InboxItem),
    Removed(InboxId),
    Review(Review),
}

/// Inbox items that are not archived, newest first, and the reviews still open. Updated by
/// `InboxChanged`, `InboxRemoved` and `ReviewChanged`.
pub struct InboxModel {
    api: Arc<dyn Api>,
    state: LoadState,
    started: bool,
    items: Vec<Arc<InboxItem>>,
    reviews: Vec<Arc<Review>>,
    buffer: Vec<InboxInput>,
}

impl InboxModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self { api, state: LoadState::Loading, started: false, items: Vec::new(), reviews: Vec::new(), buffer: Vec::new() }
    }

    /// Whether the first load is done.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Items that are not archived, newest first.
    pub fn items(&self) -> &[Arc<InboxItem>] {
        &self.items
    }

    /// One item by id.
    pub fn item(&self, id: &InboxId) -> Option<&Arc<InboxItem>> {
        self.items.iter().find(|item| &item.id == id)
    }

    /// Items a person has to act on ("4 waiting").
    pub fn waiting(&self) -> usize {
        self.items.iter().filter(|item| item.needs_person()).count()
    }

    /// The review queue: items waiting for a person, newest first as the Inbox lists them;
    /// items of the same time in the order of their ids.
    pub fn waiting_items(&self) -> Vec<Arc<InboxItem>> {
        let mut items: Vec<Arc<InboxItem>> = self.items.iter().filter(|item| item.needs_person()).cloned().collect();
        items.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.as_str().cmp(b.id.as_str())));
        items
    }

    /// Open reviews, oldest first.
    pub fn reviews(&self) -> &[Arc<Review>] {
        &self.reviews
    }

    /// One open review by id.
    pub fn review(&self, id: &ReviewId) -> Option<&Arc<Review>> {
        self.reviews.iter().find(|review| &review.id == id)
    }

    /// Reads items and open reviews, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        query(
            cx,
            &self.api,
            |api| Ok((api.inbox(false)?, api.open_reviews()?)),
            |this: &mut Self, result, cx| {
                match result {
                    Ok((items, reviews)) => {
                        this.items = items.into_iter().map(Arc::new).collect();
                        this.reviews = reviews.into_iter().map(Arc::new).collect();
                        this.state = LoadState::Ready;
                        let buffered = std::mem::take(&mut this.buffer);
                        this.apply_inputs(buffered);
                    }
                    Err(error) => this.state = failed(&error),
                }
                cx.notify();
            },
        );
    }

    pub(crate) fn apply(&mut self, inputs: Vec<InboxInput>, cx: &mut Context<Self>) {
        if !self.started {
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(inputs);
            return;
        }
        self.apply_inputs(inputs);
        cx.notify();
    }

    fn apply_inputs(&mut self, inputs: Vec<InboxInput>) {
        for input in inputs {
            match input {
                InboxInput::Item(item) => {
                    self.items.retain(|known| known.id != item.id);
                    if !item.archived {
                        self.items.push(Arc::new(item));
                    }
                }
                InboxInput::Removed(id) => self.items.retain(|known| known.id != id),
                InboxInput::Review(review) => {
                    self.reviews.retain(|known| known.id != review.id);
                    if review.state == ReviewState::Open {
                        self.reviews.push(Arc::new(review));
                    }
                }
            }
        }
        self.items.sort_by_key(|item| std::cmp::Reverse(item.created));
        self.reviews.sort_by_key(|review| review.created);
    }
}

/// One opened review. Diffs are read per variant and file, when first asked for.
pub struct ReviewModel {
    id: ReviewId,
    api: Arc<dyn Api>,
    review: Loadable<Review>,
    diffs: HashMap<(u32, String), Loadable<FileDiff>>,
}

impl ReviewModel {
    pub(crate) fn new(id: ReviewId, api: Arc<dyn Api>) -> Self {
        Self { id, api, review: Loadable::default(), diffs: HashMap::new() }
    }

    /// The review this model shows.
    pub fn id(&self) -> &ReviewId {
        &self.id
    }

    /// Where the review stands.
    pub fn state(&self) -> &LoadState {
        self.review.state()
    }

    /// The review, once loaded.
    pub fn review(&self) -> Option<&Arc<Review>> {
        self.review.value()
    }

    /// The diff of one file of one variant, once [`Self::ensure_diff`] asked for it.
    pub fn diff(&self, variant: u32, path: &str) -> Option<&Loadable<FileDiff>> {
        self.diffs.get(&(variant, path.to_string()))
    }

    /// Reads the review, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.review.started() {
            return;
        }
        let id = self.id.clone();
        let api = self.api.clone();
        fetch(self, cx, &api, |this: &mut Self| &mut this.review, move |api| {
            api.review(&id)?.ok_or_else(|| anyhow!("This review no longer exists"))
        });
    }

    /// Reads a variant's diff of `path` unless it is loaded or loading.
    pub fn ensure_diff(&mut self, variant: u32, path: &str, cx: &mut Context<Self>) {
        let key = (variant, path.to_string());
        if self.diffs.get(&key).is_some_and(Loadable::started) {
            return;
        }
        let id = self.id.clone();
        let path = path.to_string();
        let api = self.api.clone();
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.diffs.entry(key.clone()).or_default(),
            move |api| api.variant_diff(&id, variant, &path),
        );
    }

    pub(crate) fn apply(&mut self, review: Review, cx: &mut Context<Self>) {
        if !self.review.started() {
            return;
        }
        let variants_changed = self.review.value().is_none_or(|known| known.variants != review.variants);
        if variants_changed {
            self.diffs.clear();
        }
        self.review.set(review);
        cx.notify();
    }
}
