//! Load states and the one way a model runs a query: on the background executor, with the
//! result applied on the foreground only if no newer load was started meanwhile.

use std::sync::Arc;

use anyhow::Result;
use gpui::{AppContext as _, Context, SharedString};

use crate::api::Api;

/// Where a piece of data stands, so a view can draw a placeholder or an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LoadState {
    /// Not there yet: draw a placeholder.
    #[default]
    Loading,
    Ready,
    /// The query failed; the message is shown in place of the data.
    Failed(SharedString),
}

/// A value read from the engine plus its load state. A reload keeps the old value visible
/// (and the state `Ready`) until the new one arrives.
#[derive(Debug)]
pub struct Loadable<T> {
    value: Option<Arc<T>>,
    state: LoadState,
    generation: u64,
    started: bool,
}

impl<T> Default for Loadable<T> {
    fn default() -> Self {
        Self { value: None, state: LoadState::Loading, generation: 0, started: false }
    }
}

impl<T> Loadable<T> {
    /// The latest value, if one ever arrived.
    pub fn value(&self) -> Option<&Arc<T>> {
        self.value.as_ref()
    }

    /// Where the value stands.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// A value arrived and no load failed since.
    pub fn is_ready(&self) -> bool {
        self.state == LoadState::Ready
    }

    /// A load was asked for at least once.
    pub fn started(&self) -> bool {
        self.started
    }

    /// Starts a load and returns its generation.
    pub(crate) fn begin(&mut self) -> u64 {
        self.started = true;
        self.generation += 1;
        if self.value.is_none() {
            self.state = LoadState::Loading;
        }
        self.generation
    }

    /// Applies a result; `false` when a newer load superseded it.
    pub(crate) fn finish(&mut self, generation: u64, result: Result<T>) -> bool {
        if generation != self.generation {
            return false;
        }
        match result {
            Ok(value) => {
                self.value = Some(Arc::new(value));
                self.state = LoadState::Ready;
            }
            Err(error) => self.state = LoadState::Failed(format!("{error:#}").into()),
        }
        true
    }

    /// Replaces the value from an event.
    pub(crate) fn set(&mut self, value: T) {
        self.value = Some(Arc::new(value));
        self.state = LoadState::Ready;
    }

    /// Edits the value in place (cloning it only when a view still holds the old one).
    pub(crate) fn edit(&mut self, f: impl FnOnce(&mut T)) -> bool
    where
        T: Clone,
    {
        match self.value.as_mut() {
            Some(value) => {
                f(Arc::make_mut(value));
                true
            }
            None => false,
        }
    }
}

/// Runs `query` off the UI thread and stores its result in the slot `slot` picks, notifying
/// the model when it lands. A newer fetch into the same slot wins over an older one.
pub(crate) fn fetch<M, T>(
    model: &mut M,
    cx: &mut Context<M>,
    api: &Arc<dyn Api>,
    slot: impl Fn(&mut M) -> &mut Loadable<T> + 'static,
    query: impl FnOnce(&dyn Api) -> Result<T> + Send + 'static,
) where
    M: 'static,
    T: Send + 'static,
{
    let generation = slot(model).begin();
    let api = api.clone();
    let work = cx.background_spawn(async move { query(&*api) });
    cx.spawn(async move |this, cx| {
        let result = work.await;
        this.update(cx, |model, cx| {
            if slot(model).finish(generation, result) {
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

/// Runs `query` off the UI thread and hands the result to `apply` on the foreground, for
/// loads that fill more than one field.
pub(crate) fn query<M, R>(
    cx: &mut Context<M>,
    api: &Arc<dyn Api>,
    query: impl FnOnce(&dyn Api) -> Result<R> + Send + 'static,
    apply: impl FnOnce(&mut M, Result<R>, &mut Context<M>) + 'static,
) where
    M: 'static,
    R: Send + 'static,
{
    let api = api.clone();
    let work = cx.background_spawn(async move { query(&*api) });
    cx.spawn(async move |this, cx| {
        let result = work.await;
        this.update(cx, |model, cx| apply(model, result, cx)).ok();
    })
    .detach();
}

/// The text of a failed load, for `LoadState::Failed`.
pub(crate) fn failed(error: &anyhow::Error) -> LoadState {
    LoadState::Failed(format!("{error:#}").into())
}
