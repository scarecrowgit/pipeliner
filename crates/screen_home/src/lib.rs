//! Home: what needs the person, what is working right now, and how the work has been going —
//! spend, runs and agents over a period.
//!
//! The crate gives the app one view, [`HomeView`], for `Route::Home`; the shell mounts it and
//! draws its header.
//!
//! Everything shown comes from `hub` models; rows lead elsewhere through the router.

mod blocks;
mod needs;
mod period;
mod runtimes;
mod spend;
mod stats;
mod view;
mod working;

#[cfg(test)]
mod tests;

pub use view::HomeView;
