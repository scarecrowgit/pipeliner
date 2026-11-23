//! Resolving a model-supplied path inside the workdir: the security boundary of the file tools.
//!
//! A path is relative to the root, or absolute but inside it. `..` may not climb above the
//! root, and every symlink on the way is followed and must land inside the root, whether the
//! final target exists yet or not. What comes back is the real path to touch, with every
//! existing symlink already resolved, so the caller never follows a link itself.

use std::ffi::OsString;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// A path inside the workdir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The real filesystem path: the canonical root plus the remaining parts, with every
    /// symlink in the existing part resolved.
    pub abs: PathBuf,
    /// The path as people and the model read it: relative to the root, `/`-separated, `.` for
    /// the root itself. Non-UTF-8 parts are shown lossily.
    pub rel: String,
}

impl Resolved {
    /// Whether this is the root itself.
    pub fn is_root(&self) -> bool {
        self.rel == "."
    }
}

/// Why a path was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxError {
    /// The workdir itself is missing or not a directory.
    BadRoot(PathBuf),
    /// An absolute path that is not under the workdir.
    Outside(String),
    /// A relative path whose `..` climbs above the workdir.
    Escapes(String),
    /// A symlink on the way points outside the workdir.
    SymlinkOutside(String),
    /// A symlink on the way points nowhere, so its target cannot be checked.
    Dangling(String),
    /// The filesystem refused to say what is there.
    Io(String, String),
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRoot(root) => write!(f, "The workspace root {} is not a directory", root.display()),
            Self::Outside(p) => {
                write!(f, "{p} is outside the workspace. Use a path relative to the workspace root")
            }
            Self::Escapes(p) => write!(f, "{p} climbs above the workspace root. Use a path inside the workspace"),
            Self::SymlinkOutside(p) => {
                write!(f, "{p} goes through a symlink that points outside the workspace; it cannot be used")
            }
            Self::Dangling(p) => write!(f, "{p} goes through a symlink whose target does not exist"),
            Self::Io(p, e) => write!(f, "Cannot access {p}: {e}"),
        }
    }
}

impl std::error::Error for SandboxError {}

/// The canonical form of the workdir, or the error a tool should report.
pub fn canonical_root(root: &Path) -> Result<PathBuf, SandboxError> {
    match std::fs::canonicalize(root) {
        Ok(c) if c.is_dir() => Ok(c),
        _ => Err(SandboxError::BadRoot(root.to_path_buf())),
    }
}

/// Resolves `input` inside `root`. See the module docs for the rules.
pub fn resolve(root: &Path, input: &str) -> Result<Resolved, SandboxError> {
    let croot = canonical_root(root)?;
    let shown = input.trim();
    let given = Path::new(shown);

    // An absolute path must name something under the root, in its canonical or given form.
    let relative = if given.is_absolute() {
        match given.strip_prefix(&croot).or_else(|_| given.strip_prefix(root)) {
            Ok(rest) => rest.to_path_buf(),
            Err(_) => return Err(SandboxError::Outside(shown.to_string())),
        }
    } else {
        given.to_path_buf()
    };

    // Lexical normalisation first: `a/../../b` is refused before anything is looked up.
    let mut parts: Vec<OsString> = Vec::new();
    for comp in relative.components() {
        match comp {
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(SandboxError::Escapes(shown.to_string()));
                }
            }
            Component::RootDir | Component::Prefix(_) => return Err(SandboxError::Outside(shown.to_string())),
        }
    }

    // Walk down from the root. Each existing symlink is resolved completely (chains
    // included) and must stay inside; below the deepest existing ancestor nothing can be a
    // link, so the rest is appended as is.
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
                Ok(target) if target.starts_with(&croot) => cur = target,
                Ok(_) => return Err(SandboxError::SymlinkOutside(shown.to_string())),
                Err(e) if e.kind() == ErrorKind::NotFound => return Err(SandboxError::Dangling(shown.to_string())),
                Err(e) => return Err(SandboxError::Io(shown.to_string(), e.to_string())),
            },
            Ok(_) => cur = next,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                missing = true;
                cur = next;
            }
            Err(e) => return Err(SandboxError::Io(shown.to_string(), e.to_string())),
        }
    }

    let rel = if parts.is_empty() {
        ".".to_string()
    } else {
        parts.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("/")
    };
    Ok(Resolved { abs: cur, rel })
}

/// `abs` relative to the canonical root, `/`-separated, `.` for the root; the full path when
/// it is not under the root.
pub fn rel_display(croot: &Path, abs: &Path) -> String {
    match abs.strip_prefix(croot) {
        Ok(rest) if rest.as_os_str().is_empty() => ".".to_string(),
        Ok(rest) => rest.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
        Err(_) => abs.to_string_lossy().into_owned(),
    }
}

/// The lexical normal form of a path as the model wrote it, for permission matching: no `.`
/// parts, `..` applied, `/`-separated, `.` for the root. Absolute paths and paths that climb
/// above the root come back trimmed but otherwise unchanged; they never match a rule prefix
/// written for a path inside.
pub fn normalize_subject(input: &str) -> String {
    let shown = input.trim();
    let path = Path::new(shown);
    if path.is_absolute() {
        return shown.to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return shown.to_string();
                }
            }
            Component::RootDir | Component::Prefix(_) => return shown.to_string(),
        }
    }
    if parts.is_empty() { ".".to_string() } else { parts.join("/") }
}
