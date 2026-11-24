//! Where the app is: the current screen with its parameters, a short history, and the global
//! overlays that sit above every screen.

use gpui::Context;

use crate::api::{InboxId, IssueKey, NodeId, PipelineId, ReviewId, Run, RunnableRef, SessionId, Target};

/// How many screens `back` can return through.
const HISTORY: usize = 50;

/// What the Inbox opens on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InboxFocus {
    Item(InboxId),
    Review(ReviewId),
}

/// A screen and what it shows. Parameters replace one-shot "focus this" fields: navigating to
/// `Issues { issue: Some(key) }` opens that issue, and going back restores the previous screen
/// exactly as it was asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Route {
    #[default]
    Home,
    Inbox {
        item: Option<InboxFocus>,
    },
    /// Every chat, as a list.
    Chats,
    Chat {
        session: SessionId,
    },
    /// The new-chat page: pick an agent and a folder.
    NewChat,
    Issues {
        issue: Option<IssueKey>,
    },
    Agents {
        runnable: Option<RunnableRef>,
    },
    Editor {
        pipeline: PipelineId,
        node: Option<NodeId>,
    },
}

impl Route {
    /// Where a Home row (or any [`Target`]) leads. A run opens its issue when it has one, else
    /// its pipeline; pass the run when the target is one, `None` when it is not known yet.
    pub fn for_target(target: &Target, run: Option<&Run>) -> Option<Route> {
        Some(match target {
            Target::Session(session) => Route::Chat { session: session.clone() },
            Target::Issue(key) => Route::Issues { issue: Some(key.clone()) },
            Target::Review(review) => Route::Inbox { item: Some(InboxFocus::Review(review.clone())) },
            Target::Inbox(item) => Route::Inbox { item: Some(InboxFocus::Item(item.clone())) },
            Target::Run(_) => {
                let run = run?;
                match &run.issue {
                    Some(key) => Route::Issues { issue: Some(key.clone()) },
                    None => Route::Editor { pipeline: run.pipeline.clone(), node: None },
                }
            }
        })
    }

    /// The sidebar entry this route belongs to.
    pub fn section(&self) -> Section {
        match self {
            Route::Home => Section::Home,
            Route::Inbox { .. } => Section::Inbox,
            Route::Chats | Route::Chat { .. } | Route::NewChat => Section::Chats,
            Route::Issues { .. } => Section::Issues,
            Route::Agents { .. } | Route::Editor { .. } => Section::Agents,
        }
    }
}

/// The primary navigation entries of the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Home,
    Inbox,
    Chats,
    Issues,
    Agents,
}

/// A window-wide layer that can be open over any screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Overlay {
    /// The chat search and command palette (⌘K).
    Palette,
}

/// The current route, history and open overlays. Views observe it; changing it notifies.
#[derive(Debug, Default)]
pub struct Router {
    current: Route,
    back: Vec<Route>,
    /// Routes `back` left, most recent last; cleared by a new navigation.
    forward: Vec<Route>,
    overlays: Vec<Overlay>,
}

impl Router {
    /// A router showing `start`, with no history.
    pub fn new(start: Route) -> Self {
        Self { current: start, back: Vec::new(), forward: Vec::new(), overlays: Vec::new() }
    }

    /// The screen shown now.
    pub fn current(&self) -> &Route {
        &self.current
    }

    /// Goes to `route`, remembering the current one. Going where we already are does nothing.
    /// Global overlays close on navigation.
    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == self.current {
            if !self.overlays.is_empty() {
                self.overlays.clear();
                cx.notify();
            }
            return;
        }
        let previous = std::mem::replace(&mut self.current, route);
        self.back.push(previous);
        if self.back.len() > HISTORY {
            self.back.remove(0);
        }
        self.forward.clear();
        self.overlays.clear();
        cx.notify();
    }

    /// Replaces the current route without adding a history entry (a selection inside a screen).
    pub fn replace(&mut self, route: Route, cx: &mut Context<Self>) {
        if route != self.current {
            self.current = route;
            cx.notify();
        }
    }

    /// There is a route `back` returns to.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// There is a route `forward` returns to.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Returns to the previous route; `false` when there is none.
    pub fn back(&mut self, cx: &mut Context<Self>) -> bool {
        match self.back.pop() {
            Some(previous) => {
                let left = std::mem::replace(&mut self.current, previous);
                self.forward.push(left);
                self.overlays.clear();
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Undoes the last `back`; `false` when there is nothing to return to.
    pub fn forward(&mut self, cx: &mut Context<Self>) -> bool {
        match self.forward.pop() {
            Some(next) => {
                let left = std::mem::replace(&mut self.current, next);
                self.back.push(left);
                self.overlays.clear();
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// The overlay is open.
    pub fn is_open(&self, overlay: Overlay) -> bool {
        self.overlays.contains(&overlay)
    }

    /// The topmost open overlay.
    pub fn top_overlay(&self) -> Option<Overlay> {
        self.overlays.last().copied()
    }

    /// Opens an overlay above the current screen.
    pub fn open_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        if !self.overlays.contains(&overlay) {
            self.overlays.push(overlay);
            cx.notify();
        }
    }

    /// Closes an overlay if it is open.
    pub fn close_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        let before = self.overlays.len();
        self.overlays.retain(|open| *open != overlay);
        if self.overlays.len() != before {
            cx.notify();
        }
    }

    /// Opens a closed overlay or closes an open one (a shortcut's action).
    pub fn toggle_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        if self.is_open(overlay) {
            self.close_overlay(overlay, cx);
        } else {
            self.open_overlay(overlay, cx);
        }
    }
}
