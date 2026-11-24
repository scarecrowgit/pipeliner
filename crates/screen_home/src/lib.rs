//! Home: what needs the person, what is working right now, and how the work has been going —
//! spend, runs and agents over a period.
//!
//! The crate gives the app one view, [`HomeView`], for `Route::Home`; the shell mounts it and
//! draws its header.
//!
//! Everything shown comes from `hub` models; rows lead elsewhere through the router.

mod view;

pub use view::HomeView;
