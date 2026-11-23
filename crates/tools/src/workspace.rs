//! Facts about a folder that the agent loop needs: the file list for `@` mentions, the
//! project's notes for agents, and a short summary for the system prompt. No git here.

use std::fmt::Write as _;
use std::path::Path;

use base::fmt::{count, plural};

use crate::sandbox::{canonical_root, rel_display};
use crate::text::walker;

/// Files looked at, at most, when listing a workspace.
const FILES_SCAN_MAX: usize = 50_000;
/// Bytes of `AGENTS.md` or `CLAUDE.md` put into the context.
pub const NOTES_MAX_BYTES: usize = 16 * 1024;
/// Top-level entries named in the summary.
const SUMMARY_MAX_ENTRIES: usize = 60;
/// Names of the notes files, in order of preference.
const NOTES_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

/// Up to `limit` files under `root`, relative and `/`-separated, sorted by path. Files that
/// `.gitignore` excludes, hidden files and `.git` are left out. Empty when `root` is not a
/// folder.
pub fn list_files(root: &Path, limit: usize) -> Vec<String> {
    let Ok(croot) = canonical_root(root) else { return Vec::new() };
    if limit == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in walker(&croot, false).build().flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            out.push(rel_display(&croot, entry.path()));
            if out.len() >= FILES_SCAN_MAX {
                break;
            }
        }
    }
    out.sort();
    out.truncate(limit);
    out
}

/// The project's notes for agents: `AGENTS.md`, else `CLAUDE.md`, at the root, trimmed and cut
/// to [`NOTES_MAX_BYTES`] with a line saying so. `None` when neither exists or both are empty.
pub fn project_notes(root: &Path) -> Option<String> {
    let croot = canonical_root(root).ok()?;
    for name in NOTES_FILES {
        let Ok(bytes) = std::fs::read(croot.join(name)) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if text.len() <= NOTES_MAX_BYTES {
            return Some(text.to_string());
        }
        let mut end = NOTES_MAX_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        return Some(format!(
            "{}\n\n[{name} is longer than {}; read_file it for the rest.]",
            &text[..end],
            base::fmt::bytes(NOTES_MAX_BYTES as u64)
        ));
    }
    None
}

/// A few lines about the folder for the system prompt: its path and its top-level entries
/// (folders first) with counts. Ignored entries and `.git` are left out.
pub fn context_summary(root: &Path) -> String {
    let Ok(croot) = canonical_root(root) else {
        return format!("Workspace: {} (not a folder)\n", root.display());
    };
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for entry in walker(&croot, true).max_depth(Some(1)).build().flatten() {
        if entry.depth() != 1 {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();
    let mut out = format!("Workspace: {}\n", croot.display());
    let _ = writeln!(
        out,
        "Top level: {}, {}",
        plural(dirs.len() as u64, "folder", "folders"),
        plural(files.len() as u64, "file", "files")
    );
    let total = dirs.len() + files.len();
    for name in dirs.iter().chain(&files).take(SUMMARY_MAX_ENTRIES) {
        let _ = writeln!(out, "  {name}");
    }
    if total > SUMMARY_MAX_ENTRIES {
        let _ = writeln!(out, "  … and {} more", count((total - SUMMARY_MAX_ENTRIES) as u64));
    }
    let all = walker(&croot, false)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .take(FILES_SCAN_MAX)
        .count();
    let more = if all >= FILES_SCAN_MAX { "+" } else { "" };
    let _ = writeln!(out, "Files in all (not counting ignored and hidden ones): {}{more}", count(all as u64));
    out
}
