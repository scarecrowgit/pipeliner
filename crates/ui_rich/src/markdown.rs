//! Markdown for agent and user text.
//!
//! - [`parse`] turns Markdown (finished or still streaming) into a [`Document`]: a block tree
//!   of paragraphs, headings, lists, quotes, code blocks, tables and rules with inline runs.
//!   It is GPUI-free; [`parse_cached`] keeps parsed documents keyed by their text.
//! - [`Markdown`] is the view: the Graphite type scale, inline code and `@mention` boxes,
//!   links with an `on_link` callback, code blocks with a copy button, tables that scroll
//!   sideways, and mouse selection across blocks copied with Cmd/Ctrl+C ([`bind_keys`]).
//!   [`Markdown::verbatim`] shows any other text (errors, tool output, log lines) exactly as
//!   given with the same selection and copy, in the type style and colour the caller picks.
//! - [`SelectionGroup`] lets several views (log lines, list rows) share one selection: a
//!   drag across them selects through all of them, a copy joins their parts.
//! - [`plain_text`] and [`first_paragraph`] strip the Markdown for previews and list rows.

mod cache;
pub(crate) mod element;
mod group;
mod layout;
mod parse;
mod render;
mod rich;
pub(crate) mod selection;
mod view;
mod wrap;

#[cfg(test)]
mod tests;

pub use cache::{ParseCache, parse_cached};
pub use parse::{Align, Block, Document, Inline, Item, List, Style, Table, parse};
pub use rich::{first_paragraph, plain_text};
pub use group::SelectionGroup;
pub use view::{Copy, KEY_CONTEXT, LinkHandler, Markdown, SelectAll, bind_keys};
