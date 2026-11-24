//! The Inbox: reviews that wait for a person (variants side by side, diffs, renders, ship /
//! skip / send back), questions from agents, reports and pull requests.
//!
//! The crate gives the app one view, [`InboxView`], for `Route::Inbox`; the shell mounts it and
//! draws its header.
//!
//! Everything shown comes from `hub` models; everything done goes through `hub::commands`.

mod files;
mod keys;
mod list;
mod other;
mod review;
mod select;
mod send_back;
mod table;
mod texts;
mod view;
mod waits;

#[cfg(test)]
mod tests;

pub use view::InboxView;
