//! The GPUI bridge between the domain and the views.
//!
//! Screens never call the engine directly. They get everything from here:
//!
//! - **Models** ([`ChatIndex`], [`SessionModel`], [`SearchModel`], [`IssuesModel`], [`RunsModel`],
//!   [`RunModel`], [`InboxModel`], [`ReviewModel`], [`CatalogModel`], [`OpsModel`],
//!   [`PermissionsModel`]): GPUI entities a view observes. Each loads on first use, then stays
//!   current through engine events, notifying at most once per batch of events.
//! - [`Commands`]: a cheap handle whose methods make things happen without blocking the UI
//!   thread, reaching the engine in the order they were called. Failures come back as a
//!   [`HubNotice`] emitted by the [`Hub`].
//! - [`Queries`]: one-off reads (workdir pickers, model lists, estimates) run off the UI thread.
//! - [`Router`]: the current [`Route`], history, and the open global overlays.
//! - [`fixtures`]: an in-memory [`api::Api`] for previews and tests.
//!
//! A view that shows the issue board:
//!
//! ```ignore
//! let issues = hub::issues(cx);                        // loads on first use
//! cx.observe(&issues, |_, _, cx| cx.notify()).detach(); // re-render on change
//! let board = issues.read(cx).column(IssueStatus::Todo);
//! hub::commands(cx).start_issue(&key, cx);             // never blocks
//! hub::navigate(Route::Issues { issue: Some(key) }, cx);
//! ```

pub mod commands;
mod commands_more;
pub mod fixtures;
mod hub;
mod load;
pub mod models;
mod notice;
mod pump;
pub mod queries;
pub mod router;
pub mod transcript;

#[cfg(test)]
mod tests;

/// The engine's contract: every query, command and type the app uses.
pub use engine::api;
pub use model;

pub use commands::Commands;
pub use hub::{
    Hub, catalog, chat_index, commands, display_path, home, hub, inbox, issues, navigate, open, ops, permissions, queries, review,
    router, run, runs, search, session,
};
pub use load::{LoadState, Loadable};
pub use models::{
    CatalogModel, ChatIndex, ContextUsage, InboxModel, IssuesModel, OpsModel, PermissionsModel, ReviewModel, RunLogEvent,
    RunModel, RunsModel, SearchModel, SessionEvent, SessionModel,
};
pub use notice::HubNotice;
pub use queries::Queries;
pub use router::{InboxFocus, Overlay, Route, Router, Section};
pub use transcript::{Block, BlockKind, Splice};
