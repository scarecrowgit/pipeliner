//! Resolving model-supplied paths inside the workspace root.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// A path inside the workspace: `abs` is the resolved filesystem path (symlinks in the
/// existing part resolved), `rel` the workspace-relative display form (`/`-separated,
/// `.` for the root).
#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    pub abs: PathBuf,
    pub rel: String,
}

/// Canonical root (falls back to the raw root if it can't be canonicalized).
pub(crate) fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Resolve `input` relative to `root`. Rejects absolute paths outside the root, `..` that
/// escapes it, and symlinks pointing outside. Works for paths that don't exist yet.
pub(crate) fn resolve(root: &Path, input: &str) -> Result<Resolved, String> {
    let croot = canonical_root(root);
    if !croot.is_dir() {
        return Err(format!("Workspace root {} is not a directory", root.display()));
    }
    let trimmed = input.trim();
    let given = Path::new(trimmed);

    // Strip the root prefix from absolute paths.
    let relative: PathBuf = if given.is_absolute() {
        if let Ok(rest) = given.strip_prefix(&croot) {
            rest.to_path_buf()
        } else if let Ok(rest) = given.strip_prefix(root) {
            rest.to_path_buf()
        } else {
            return Err(format!(
                "Path {trimmed} is outside the workspace. Use a path relative to the workspace root."
            ));
        }
    } else {
        given.to_path_buf()
    };

    // Lexical normalization; `..` may not climb above the root.
    let mut parts: Vec<String> = Vec::new();
    for comp in relative.components() {
        match comp {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(format!(
                        "Path {trimmed} escapes the workspace root. Use a path inside the workspace."
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("Path {trimmed} is outside the workspace."));
            }
        }
    }

    // Walk the components, resolving symlinks in the existing part.
    let mut cur = croot.clone();
    let mut missing = false;
    for part in &parts {
        let next = cur.join(part);
        if missing {
            cur = next;
            continue;
        }
        match std::fs::symlink_metadata(&next) {
            Ok(meta) if meta.file_type().is_symlink() => match std::fs::canonicalize(&next) {
                Ok(target) => {
                    if !target.starts_with(&croot) {
                        return Err(format!(
                            "Path {trimmed} goes through a symlink pointing outside the workspace."
                        ));
                    }
                    cur = target;
                }
                Err(_) => {
                    return Err(format!("Path {trimmed} goes through a dangling symlink."));
                }
            },
            Ok(_) => cur = next,
            Err(e) if e.kind() == ErrorKind::NotFound || e.kind() == ErrorKind::NotADirectory => {
                missing = true;
                cur = next;
            }
            Err(e) => return Err(format!("Cannot access {trimmed}: {e}")),
        }
    }

    let rel = if parts.is_empty() { ".".to_string() } else { parts.join("/") };
    Ok(Resolved { abs: cur, rel })
}

/// Workspace-relative `/`-separated form of an absolute path under `croot`.
pub(crate) fn rel_display(croot: &Path, abs: &Path) -> String {
    match abs.strip_prefix(croot) {
        Ok(rest) => {
            let parts: Vec<String> =
                rest.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            if parts.is_empty() { ".".to_string() } else { parts.join("/") }
        }
        Err(_) => abs.to_string_lossy().into_owned(),
    }
}
