//! Keeping the detail on the selected row of the visible list: what opens when nothing is
//! focused, which tab a focused item is shown in, and where the selection goes when the
//! open item leaves the list.

use std::sync::Arc;

use gpui::Context;
use hub::api::InboxItem;
use hub::{InboxFocus, LoadState, Route};

use crate::list::Tab;
use crate::view::InboxView;

/// What the next reconciliation does besides keeping the open row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pending {
    /// The list changed: an open item that left the tab is followed to the live tab that
    /// has it, or the row now at its place opens.
    #[default]
    Keep,
    /// The route named the item: show the tab that has it, the archive included.
    Follow,
    /// The person picked a tab: open its first row unless the open item is in it.
    First,
}

impl InboxView {
    /// Makes the detail the selected row of the visible list.
    pub(crate) fn reconcile(&mut self, cx: &mut Context<Self>) {
        // A tab switch asks for one more look at the new tab's rows.
        for _ in 0..4 {
            if !self.reconcile_step(cx) {
                break;
            }
        }
    }

    /// One look at the list; `true` when the tab changed.
    fn reconcile_step(&mut self, cx: &mut Context<Self>) -> bool {
        if self.inbox.read(cx).state() != &LoadState::Ready {
            return false;
        }
        if self.tab == Tab::Archived && self.archived.state != LoadState::Ready {
            if !self.archived.started {
                self.load_archived(cx);
            }
            return false;
        }
        let rows = self.visible_items(cx);
        let open_at = rows.iter().position(|item| self.is_open(item));
        if let Some(at) = open_at {
            self.row = at;
            self.pending = Pending::Keep;
            return false;
        }
        if self.pending == Pending::First || self.focus.is_none() {
            if rows.is_empty() && self.tab == Tab::Waiting && !self.user_tab {
                self.tab = Tab::All;
                return true;
            }
            self.pending = Pending::Keep;
            self.select(rows.first().cloned(), cx);
            return false;
        }
        // The open item is not in this tab.
        let live = [Tab::Waiting, Tab::All].into_iter().find(|tab| {
            self.live_items(*tab, cx)
                .iter()
                .any(|item| self.is_open(item))
        });
        if let Some(tab) = live {
            self.tab = tab;
            return true;
        }
        if self.pending == Pending::Follow {
            if !self.archived.started || self.archived.state == LoadState::Loading {
                if !self.archived.started {
                    self.load_archived(cx);
                }
                return false;
            }
            if self.archived.items.iter().any(|item| self.is_open(item)) {
                self.tab = Tab::Archived;
                return true;
            }
        }
        // It is gone (decided, archived elsewhere, unknown): the row now at its place opens.
        self.pending = Pending::Keep;
        if rows.is_empty() && self.tab == Tab::Waiting && !self.user_tab {
            self.tab = Tab::All;
            self.focus = None;
            return true;
        }
        let at = self.row.min(rows.len().saturating_sub(1));
        self.select(rows.get(at).cloned(), cx);
        false
    }

    /// Opens `item` (or nothing) and lets the route follow.
    fn select(&mut self, item: Option<Arc<InboxItem>>, cx: &mut Context<Self>) {
        let focus = item.map(|item| InboxFocus::Item(item.id.clone()));
        if focus == self.focus {
            return;
        }
        self.focus = focus.clone();
        if matches!(hub::router(cx).read(cx).current(), Route::Inbox { .. }) {
            hub::router(cx).update(cx, |router, cx| {
                router.replace(Route::Inbox { item: focus }, cx)
            });
        }
        self.sync_review(cx);
        self.sync_item(cx);
        cx.notify();
    }
}
