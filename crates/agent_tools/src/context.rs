//! Project context: git branch, top-level entries, instruction files, file list.

use std::path::{Path, PathBuf};

use crate::paths::{canonical_root, rel_display};
use crate::tools::{SKIP_DIRS, dir_entries};

const INSTRUCTIONS_MAX_BYTES: usize = 8 * 1024;
const TOP_LEVEL_MAX: usize = 100;
const FILES_SCAN_MAX: usize = 50_000;

/// Find the git dir for `root` (walking up), following `.git` files (worktrees/submodules).
fn git_dir(root: &Path) -> Option<PathBuf> {
    for dir in root.ancestors() {
        let dot = dir.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            let p = Path::new(target);
            return Some(if p.is_absolute() { p.to_path_buf() } else { dir.join(p) });
        }
    }
    None
}

/// Current branch from `.git/HEAD`; short hash when detached.
pub(crate) fn git_branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir(&canonical_root(root))?.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string())
    } else if head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(head[..7].to_string())
    } else {
        None
    }
}

fn truncate_bytes(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

pub(crate) fn context_summary(root: &Path) -> String {
    let croot = canonical_root(root);
    let mut out = format!("Workspace root: {}\n", croot.display());
    if let Some(branch) = git_branch(root) {
        out.push_str(&format!("Git branch: {branch}\n"));
    }
    if let Ok(entries) = dir_entries(&croot) {
        out.push_str("\nTop-level entries:\n");
        for e in entries.iter().take(TOP_LEVEL_MAX) {
            out.push_str(&format!("  {e}\n"));
        }
        if entries.len() > TOP_LEVEL_MAX {
            out.push_str(&format!("  ... and {} more\n", entries.len() - TOP_LEVEL_MAX));
        }
    }
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let Ok(text) = std::fs::read_to_string(croot.join(name)) else { continue };
        let (text, cut) = truncate_bytes(text.trim(), INSTRUCTIONS_MAX_BYTES);
        out.push_str(&format!("\n--- {name} ---\n{text}\n"));
        if cut {
            out.push_str(&format!("[{name} truncated; read_file it for the rest]\n"));
        }
    }
    out
}

pub(crate) fn files(root: &Path, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let croot = canonical_root(root);
    let mut builder = ignore::WalkBuilder::new(&croot);
    builder.hidden(true).git_ignore(true).git_exclude(true).parents(true).require_git(false);
    builder.filter_entry(|e| {
        e.depth() == 0 || !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
    });
    let mut out = Vec::new();
    for entry in builder.build().flatten() {
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

pub(crate) fn repo_label(root: &Path) -> (String, Option<String>) {
    let croot = canonical_root(root);
    let name = croot
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| croot.display().to_string());
    (name, git_branch(root))
}
