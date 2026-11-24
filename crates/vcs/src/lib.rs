//! Git facts, worktrees, branches, clone, diffs, merges and pull requests through gh.
//!
//! Everything goes through the `git` and `gh` command lines, run with
//! [`base::process::Cmd`]: no prompts, no editors, a timeout on every command, and errors that
//! carry the command and its stderr. Functions take their paths explicitly and block, so call
//! them from the engine's own threads; nothing here reads global state.
//!
//! | Module | What it gives |
//! | --- | --- |
//! | [`facts`] | Branch, remote, dirty count and ahead/behind of a folder; the one-line status |
//! | [`worktree`] | List, create and remove linked worktrees; unique branch names |
//! | [`changes`] | Changed files and diffs against a base, commit, merge, push |
//! | [`clone`] | Cloning a repository |
//! | [`pr`] | Pull requests through `gh`: create, view, find URLs in text |
//! | [`scan`] | What a folder holds for an agent; repositories below a folder |

pub mod changes;
pub mod clone;
mod exec;
pub mod facts;
pub mod pr;
pub mod scan;
#[cfg(test)]
mod testutil;
pub mod worktree;

pub use changes::{MergeOutcome, changed_files, commit_all, file_diff, merge_into_base, parse_unified, push};
pub use facts::{RepoFacts, branches, facts, host_of, status_line};
pub use scan::{ChildDir, FolderInfo, child_dirs, folder, repos_under};
pub use worktree::{Worktree, branch_name, default_dir, unique_branch};
