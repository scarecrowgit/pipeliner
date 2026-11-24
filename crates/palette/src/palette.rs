//! The palette's state and what its keys, chips and rows do. Drawing is in `render`.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{AppContext as _, Context, Entity, ScrollHandle, Subscription, Task, Window, point, px};
use hub::api::{Millis, SearchResult, SessionHit, SessionQuery};
use hub::{Hub, LoadState, Overlay, Route, Router, SearchModel};
use ui::motion::{self, Presence};
use ui::theme::DURATION_EXIT;
use ui_rich::{TextInput, TextInputEvent};

use crate::facets::Facet;
use crate::groups::{Listing, group, local_offset};

/// Rows a page key moves when the list has not been laid out yet.
const DEFAULT_PAGE: isize = 6;
/// Height of a result row, to turn the list's height into a page.
const ROW_PX: f32 = 62.;

/// The ⌘K chat palette: a search field over the user's chats, filter chips, a list of
/// grouped results and a preview of the selected one.
///
/// Create one each time the router's [`Overlay::Palette`] opens and drop it when it closes
/// (see the crate docs). It focuses its field, asks [`SearchModel`] for results as the user
/// types, and closes itself by clearing the overlay in the router, or by navigating to the
/// chosen chat.
pub struct Palette {
    pub(crate) hub: Entity<Hub>,
    pub(crate) search: Entity<SearchModel>,
    pub(crate) router: Entity<Router>,
    pub(crate) input: Entity<TextInput>,
    /// `in:<repo>` taken out of the typed text; shown as a tag before the field.
    pub(crate) scope: Option<String>,
    /// Labels of the sections whose rows are hidden.
    pub(crate) collapsed: HashSet<String>,
    /// The results the list was built from.
    pub(crate) result: Option<Arc<SearchResult>>,
    pub(crate) listing: Listing,
    /// The selected row (not entry): index into `listing.rows`.
    pub(crate) selected: usize,
    pub(crate) scroll: ScrollHandle,
    pub(crate) presence: Presence,
    /// Set once Escape, the scrim or the close key asked to leave: input is ignored.
    pub(crate) closing: bool,
    /// The clock the sections are counted from; the wall clock unless a test sets it.
    pub(crate) now: Option<Millis>,
    exit: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Palette {
    /// A palette over `hub`'s chat search, with an empty query and its field focused.
    pub fn new(hub: &Entity<Hub>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = hub::search(cx);
        let router = hub.read(cx).router().clone();
        let input = cx.new(|cx| TextInput::new(cx).placeholder("Search chats"));
        let subscriptions = vec![
            cx.subscribe(&input, |this, _, event: &TextInputEvent, cx| this.on_input(event, cx)),
            cx.observe(&search, |this, _, cx| this.sync(cx)),
        ];
        let mut presence = Presence::default();
        presence.set(true);
        let mut palette = Self {
            hub: hub.clone(),
            search,
            router,
            input,
            scope: None,
            collapsed: HashSet::new(),
            result: None,
            listing: Listing::default(),
            selected: 0,
            scroll: ScrollHandle::new(),
            presence,
            closing: false,
            now: None,
            exit: None,
            _subscriptions: subscriptions,
        };
        palette.edit_query(|query| *query = SessionQuery::default(), cx);
        palette.result = palette.search.read(cx).result().cloned();
        palette.relayout();
        palette.focus(window, cx);
        palette
    }

    /// Moves keyboard focus into the search field.
    pub fn focus(&self, window: &mut Window, cx: &gpui::App) {
        self.input.read(cx).focus(window);
    }

    /// Counts the sections ("Today", "Yesterday"…) from `now` instead of the wall clock.
    pub fn set_now(&mut self, now: Millis, cx: &mut Context<Self>) {
        self.now = Some(now);
        self.relayout();
        cx.notify();
    }

    /// The chat the selection is on.
    pub fn selected_session(&self) -> Option<&hub::api::Session> {
        self.selected_hit().map(|hit| &hit.session)
    }

    /// The text typed in the field, without an `in:` scope.
    pub fn text<'a>(&self, cx: &'a gpui::App) -> &'a str {
        self.input.read(cx).text()
    }

    /// Where the search stands: loading, ready or failed.
    pub fn state<'a>(&self, cx: &'a gpui::App) -> &'a LoadState {
        self.search.read(cx).state()
    }

    pub(crate) fn clock(&self) -> Millis {
        self.now.unwrap_or_else(base::time::now_ms)
    }

    pub(crate) fn selected_hit(&self) -> Option<&SessionHit> {
        let hit = *self.listing.rows.get(self.selected)?;
        self.result.as_ref()?.hits.get(hit)
    }

    pub(crate) fn query<'a>(&self, cx: &'a gpui::App) -> &'a SessionQuery {
        self.search.read(cx).query()
    }

    // ------------------------------------------------------------------ results

    fn relayout(&mut self) {
        let groups = match &self.result {
            Some(result) => {
                let now = self.clock();
                group(&result.hits, now, local_offset(now))
            }
            None => Vec::new(),
        };
        self.listing = Listing::new(groups, &self.collapsed);
        self.selected = self.selected.min(self.listing.rows.len().saturating_sub(1));
    }

    /// Takes the model's latest results: a new result starts at the top.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let latest = self.search.read(cx).result().cloned();
        let same = match (&self.result, &latest) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.result = latest;
            self.selected = 0;
            self.relayout();
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        cx.notify();
    }

    /// Changes the query the model runs; it waits for the typing to pause before searching.
    fn edit_query(&mut self, change: impl FnOnce(&mut SessionQuery), cx: &mut Context<Self>) {
        let mut query = self.search.read(cx).query().clone();
        change(&mut query);
        self.search.update(cx, |model, cx| model.set_query(query, cx));
    }

    /// Runs the current query again (after a failure, or after a command changed a chat).
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.edit_query(|_| {}, cx);
    }

    fn on_input(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        match event {
            TextInputEvent::Changed => self.text_changed(cx),
            TextInputEvent::Submit => self.open_selected(cx),
            TextInputEvent::Cancel => self.close(cx),
            TextInputEvent::Up => self.move_by(-1, cx),
            TextInputEvent::Down => self.move_by(1, cx),
            TextInputEvent::Focus | TextInputEvent::Blur | TextInputEvent::Tab => {}
        }
    }

    fn text_changed(&mut self, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).text().to_string();
        let text = match take_scope(&typed) {
            Some((repo, rest)) => {
                self.scope = Some(repo);
                self.input.update(cx, |input, cx| input.set_text(&rest, cx));
                rest
            }
            None => typed,
        };
        let scope = self.scope.clone();
        self.edit_query(
            |query| {
                query.text = text;
                query.repo = scope;
            },
            cx,
        );
    }

    /// Drops the `in:` scope tag.
    pub(crate) fn clear_scope(&mut self, cx: &mut Context<Self>) {
        if self.scope.take().is_some() {
            self.edit_query(|query| query.repo = None, cx);
        }
    }

    /// Presses a filter chip.
    pub(crate) fn toggle_facet(&mut self, facet: Facet, cx: &mut Context<Self>) {
        self.edit_query(|query| facet.toggle(query), cx);
    }

    /// Opens or closes a section.
    pub(crate) fn toggle_group(&mut self, label: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(label) {
            self.collapsed.insert(label.to_string());
        }
        let selected = self.selected_hit().map(|hit| hit.session.id.clone());
        self.relayout();
        let kept = selected.and_then(|id| {
            let hits = &self.result.as_ref()?.hits;
            self.listing.rows.iter().position(|&hit| hits.get(hit).is_some_and(|h| h.session.id == id))
        });
        self.selected = kept.unwrap_or(0);
        cx.notify();
    }

    // ------------------------------------------------------------------ selection

    /// Moves the selection by `delta` rows, wrapping round at either end.
    pub(crate) fn move_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.listing.rows.len() as isize;
        if rows == 0 {
            return;
        }
        let next = (self.selected as isize + delta).rem_euclid(rows) as usize;
        self.select(next, true, cx);
    }

    /// Moves the selection a page up (`-1`) or down (`1`), stopping at the first and last row.
    pub(crate) fn page(&mut self, direction: isize, cx: &mut Context<Self>) {
        let rows = self.listing.rows.len();
        if rows == 0 {
            return;
        }
        let height = f32::from(self.scroll.bounds().size.height);
        let page = if height > 0. { ((height / ROW_PX) as isize - 1).max(1) } else { DEFAULT_PAGE };
        let next = (self.selected as isize + direction * page).clamp(0, rows as isize - 1) as usize;
        self.select(next, true, cx);
    }

    /// Selects row `row`; `reveal` scrolls it into view (not wanted while the pointer moves it).
    pub(crate) fn select(&mut self, row: usize, reveal: bool, cx: &mut Context<Self>) {
        if row >= self.listing.rows.len() || (row == self.selected && !reveal) {
            return;
        }
        self.selected = row;
        if reveal && let Some(entry) = self.listing.scroll_target(row) {
            self.scroll.scroll_to_item(entry);
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ doing things

    /// Opens the selected chat: the router goes to it and the palette closes with it.
    pub(crate) fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_hit().map(|hit| hit.session.id.clone()) else { return };
        self.router.update(cx, |router, cx| router.navigate(Route::Chat { session: id }, cx));
    }

    /// Leaves without choosing: the overlay fades out, then leaves the router.
    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.presence.set(false);
        if motion::reduced() {
            self.router.update(cx, |router, cx| router.close_overlay(Overlay::Palette, cx));
        } else {
            self.exit = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(motion::scaled(DURATION_EXIT)).await;
                this.update(cx, |this, cx| {
                    this.router.update(cx, |router, cx| router.close_overlay(Overlay::Palette, cx));
                })
                .ok();
            }));
        }
        cx.notify();
    }

    /// Pins the selected chat, or unpins it.
    pub(crate) fn pin_selected(&mut self, cx: &mut Context<Self>) {
        let Some((id, pinned)) = self.selected_hit().map(|hit| (hit.session.id.clone(), hit.session.pinned)) else { return };
        let commands = self.hub.read(cx).commands().clone();
        commands.pin_session(&id, !pinned, cx);
        self.refresh(cx);
    }

    /// Archives the selected chat.
    pub(crate) fn archive_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_hit().map(|hit| hit.session.id.clone()) else { return };
        let commands = self.hub.read(cx).commands().clone();
        commands.archive_session(&id, true, cx);
        self.refresh(cx);
    }

    /// Copies the selected chat into a new one and opens that.
    pub(crate) fn fork_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_hit().map(|hit| hit.session.id.clone()) else { return };
        let commands = self.hub.read(cx).commands().clone();
        let forked = commands.fork_session(&id, None, cx);
        let router = self.router.clone();
        cx.spawn(async move |_, cx| {
            if let Some(session) = forked.await {
                router.update(cx, |router, cx| router.navigate(Route::Chat { session: session.id }, cx)).ok();
            }
        })
        .detach();
    }
}

/// Takes a finished `in:<repo>` word out of what was typed: `("payments-api", "retry")` for
/// `in:payments-api retry`. A word still being typed (no space after it) stays where it is.
pub(crate) fn take_scope(typed: &str) -> Option<(String, String)> {
    let mut start = 0;
    for token in typed.split_inclusive(char::is_whitespace) {
        if token.ends_with(char::is_whitespace)
            && let Some(repo) = token.trim_end().strip_prefix("in:").filter(|repo| !repo.is_empty())
        {
            let rest = format!("{}{}", &typed[..start], &typed[start + token.len()..]);
            return Some((repo.to_string(), rest));
        }
        start += token.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::take_scope;

    #[test]
    fn a_finished_scope_word_leaves_the_text() {
        assert_eq!(take_scope("in:payments-api retry"), Some(("payments-api".into(), "retry".into())));
        assert_eq!(take_scope("retry in:ledger x"), Some(("ledger".into(), "retry x".into())));
        assert_eq!(take_scope("in:ledger "), Some(("ledger".into(), String::new())));
    }

    #[test]
    fn an_unfinished_or_empty_scope_word_stays() {
        assert_eq!(take_scope("in:payments"), None);
        assert_eq!(take_scope("in: retry"), None);
        assert_eq!(take_scope("within:x retry"), None);
    }
}
