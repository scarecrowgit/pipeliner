//! The issues screen: the board and the list of a project's issues, and the panel of one issue
//! with its properties, runs, activity and comments.
//!
//! The crate gives the app one view, [`IssuesView`], for `Route::Issues`; the shell mounts it
//! and draws its header. [`IssuePanel`] is the side panel the view opens for the issue in the
//! route.
//!
//! Everything shown comes from `hub` models; everything done goes through `hub::commands`.

pub mod board;
pub mod panel;

pub use board::IssuesView;
pub use panel::{IssuePanel, PanelEvent};
