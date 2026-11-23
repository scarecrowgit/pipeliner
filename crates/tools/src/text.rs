//! Small helpers the file tools share: binary sniffing, line cutting, walking with `.gitignore`.

use std::path::Path;

use model::ToolDisplay;

/// Bytes looked at to decide whether a file is binary.
const BINARY_SNIFF: usize = 8192;
/// Directories never listed or searched, even when no `.gitignore` names them.
pub(crate) const SKIP_DIRS: &[&str] = &[".git", "node_modules"];

/// A NUL byte near the start means binary, as git and ripgrep decide it.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0)
}

/// `line` without its line ending.
pub(crate) fn strip_eol(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// A walker over `path` that honours `.gitignore` (also outside a git repo), `.ignore` and
/// git's exclude file, skips [`SKIP_DIRS`], never follows symlinks and sorts by name.
pub(crate) fn walker(path: &Path, hidden: bool) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(path);
    builder
        .hidden(!hidden)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| e.depth() == 0 || !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()));
    builder
}

/// A display for a preview that cannot be made because the input is wrong.
pub(crate) fn preview_error(tool: &str, message: String) -> ToolDisplay {
    ToolDisplay::Text { title: tool.to_string(), body: message }
}
