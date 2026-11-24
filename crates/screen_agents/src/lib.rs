//! The agents screen: the library of agents, orchestrators and pipelines — what each is, what
//! it may do, what uses it and how it has been doing — with the menus and dialogs that create,
//! duplicate, import, export, archive and delete them.
//!
//! The crate gives the app one view, [`AgentsView`], for `Route::Agents`; the shell mounts it
//! and draws its header. Opening a pipeline leaves for the editor (`Route::Editor`).
//!
//! Everything shown comes from `hub` models; everything done goes through `hub::commands`.
//!
//! Areas: [`rows`] (what the list shows, as plain data), [`list`] (tab bar, search, rows),
//! [`detail`] and [`pipeline`] (the panel of the selection), [`edit`] (saving definitions and
//! the edit dialog), [`menus`] (New, more, and the panel's pickers), [`dialogs`] (delete,
//! archive toast, rename, import, export, templates).

mod detail;
mod dialogs;
mod edit;
mod list;
mod menus;
mod pipeline;
mod rows;
mod view;

#[cfg(test)]
mod tests;

pub use view::AgentsView;
