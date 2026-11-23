//! Paths, ids, clock, formatting, cancellation, the process runner and the frontmatter parser.
//!
//! The bottom of the domain layer: every other crate takes these from here instead of keeping
//! its own copy. Nothing in this crate knows about the UI, the store or a model provider.
//!
//! | Module | What it gives |
//! | --- | --- |
//! | [`paths`] | Where the app reads and writes: config, data, worktrees |
//! | [`id`] | Sortable unique ids and URL/branch-safe slugs |
//! | [`time`] | A clock that tests can pin, local calendar math, "3h" / "12 Oct" labels |
//! | [`fmt`] | The number style of the mockups: tokens, cost, durations, sizes |
//! | [`cancel`] | A cancellation flag that can be shared and nested |
//! | [`process`] | The one way to run a child process: timeout, cancel, capped output |
//! | [`frontmatter`] | The one parser for `---` headers of agent and skill Markdown files |

pub mod cancel;
pub mod fmt;
pub mod frontmatter;
pub mod id;
pub mod paths;
pub mod process;
pub mod time;
