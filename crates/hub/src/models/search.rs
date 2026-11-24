//! Chat search for the palette: debounced, and a newer query always wins.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use gpui::{AppContext as _, Context, Task};

use crate::api::{Api, SearchResult, SessionQuery};
use crate::load::{LoadState, failed};

/// Quiet time after the last keystroke before the search runs.
pub const DEBOUNCE: Duration = Duration::from_millis(120);

/// The palette's search state. Results of an older query never replace a newer one.
pub struct SearchModel {
    api: Arc<dyn Api>,
    query: SessionQuery,
    result: Option<Arc<SearchResult>>,
    state: LoadState,
    generation: u64,
    pending: Option<Task<()>>,
}

impl SearchModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self {
            api,
            query: SessionQuery::default(),
            result: None,
            state: LoadState::Loading,
            generation: 0,
            pending: None,
        }
    }

    /// The query the shown results are for, or will be once they arrive.
    pub fn query(&self) -> &SessionQuery {
        &self.query
    }

    /// The latest results; stays visible while a newer query runs.
    pub fn result(&self) -> Option<&Arc<SearchResult>> {
        self.result.as_ref()
    }

    /// Whether the newest query's results are in.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// Runs the default query (recent chats) if nothing ran yet.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.generation == 0 {
            self.set_query(self.query.clone(), cx);
        }
    }

    /// Searches for `query` after [`DEBOUNCE`]; a call before then replaces it.
    pub fn set_query(&mut self, query: SessionQuery, cx: &mut Context<Self>) {
        self.query = query.clone();
        self.generation += 1;
        let generation = self.generation;
        self.state = LoadState::Loading;
        let api = self.api.clone();
        // Dropping the previous task cancels its timer or its pending result.
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let result = cx.background_spawn(async move { api.search_sessions(&query) }).await;
            this.update(cx, |this, cx| this.finish(generation, result, cx)).ok();
        }));
        cx.notify();
    }

    /// Changes only the typed text of the current query.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let mut query = self.query.clone();
        query.text = text.into();
        self.set_query(query, cx);
    }

    /// Applies a result if it belongs to the newest query; `false` when it is stale.
    pub(crate) fn finish(&mut self, generation: u64, result: Result<SearchResult>, cx: &mut Context<Self>) -> bool {
        if generation != self.generation {
            return false;
        }
        match result {
            Ok(result) => {
                self.result = Some(Arc::new(result));
                self.state = LoadState::Ready;
            }
            Err(error) => self.state = failed(&error),
        }
        cx.notify();
        true
    }

    /// The generation of the newest query, for tests of stale results.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}
