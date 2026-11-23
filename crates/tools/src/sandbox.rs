//! Resolving a model-supplied path inside the workdir: the security boundary of the file tools.
//!
//! A path is relative to the root, or absolute but inside it. `..` may not climb above the
//! root, and every symlink on the way is followed and must land inside the root, whether the
//! final target exists yet or not. What comes back is the real path to touch, with every
//! existing symlink already resolved, so the caller never follows a link itself.
//!
//! Files are then opened through [`open_regular`] and written through [`write_regular`]: only
//! regular files (never a FIFO, socket or device, which could block forever), and a symlink
//! that appeared at the final component after [`resolve`] is refused.
//!
//! The guarantee holds against a static layout, such as a cloned repository with hostile
//! symlinks. It does not hold against a process that rearranges directories while a tool
//! runs: swapping an intermediate directory for a symlink between the check and the open, or
//! hard-linking a file from outside. Only a process started through `bash` can do that, and
//! `bash` is outside the sandbox by design; its calls go through permission rules instead.

use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// A path inside the workdir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The canonical workdir.
    pub root: PathBuf,
    /// The real filesystem path: the canonical root plus the remaining parts, with every
    /// symlink in the existing part resolved.
    pub abs: PathBuf,
    /// [`Resolved::abs`] relative to the canonical root, `/`-separated, `.` for the root
    /// itself: the path the call really touches. Non-UTF-8 parts are shown lossily.
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

    let rel = rel_display(&croot, &cur);
    Ok(Resolved { root: croot, abs: cur, rel })
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

/// The lexical normal form of a path pattern: no `.` parts, `..` applied, `/`-separated, `.`
/// for the root. `..` that climbs above the start stays in front (`src/../../x` is `../x`);
/// absolute paths come back trimmed. Rule patterns are normalised with this; subjects come
/// from [`resolve`].
pub fn normalize_subject(input: &str) -> String {
    let shown = input.trim();
    let path = Path::new(shown);
    if path.is_absolute() {
        return shown.to_string();
    }
    let (mut ups, mut parts) = (0usize, Vec::<String>::new());
    for comp in path.components() {
        match comp {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    ups += 1;
                }
            }
        }
    }
    let all: Vec<String> = std::iter::repeat_n("..".to_string(), ups).chain(parts).collect();
    if all.is_empty() { ".".to_string() } else { all.join("/") }
}

/// Why a file could not be opened or written.
#[derive(Debug)]
pub enum OpenError {
    NotFound,
    IsDir,
    /// A FIFO, socket or device.
    NotRegular,
    /// The final component became a symlink, or another file, after it was checked.
    Changed,
    Io(std::io::Error),
}

impl OpenError {
    /// The message for the model about the file shown as `rel`.
    pub fn message(&self, rel: &str) -> String {
        match self {
            Self::NotFound => format!("File not found: {rel}"),
            Self::IsDir => format!("{rel} is a directory, not a file"),
            Self::NotRegular => format!("{rel} is not a regular file (a FIFO, socket or device); it cannot be used"),
            Self::Changed => format!("{rel} changed while it was being opened (it is now a symlink or another file)"),
            Self::Io(e) => format!("Cannot access {rel}: {e}"),
        }
    }
}

fn io_error(e: std::io::Error) -> OpenError {
    match e.kind() {
        ErrorKind::NotFound | ErrorKind::NotADirectory => OpenError::NotFound,
        _ => OpenError::Io(e),
    }
}

/// What is at the final component right now, without following it.
fn check_final(abs: &Path) -> Result<std::fs::Metadata, OpenError> {
    let meta = std::fs::symlink_metadata(abs).map_err(io_error)?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        Err(OpenError::Changed)
    } else if kind.is_dir() {
        Err(OpenError::IsDir)
    } else if !kind.is_file() {
        Err(OpenError::NotRegular)
    } else {
        Ok(meta)
    }
}

/// Whether an open file is the very file `before` described.
fn same_file(file: &File, before: &std::fs::Metadata) -> Result<(), OpenError> {
    let after = file.metadata().map_err(OpenError::Io)?;
    if after.is_file() && after.dev() == before.dev() && after.ino() == before.ino() {
        Ok(())
    } else {
        Err(OpenError::Changed)
    }
}

/// Opens a resolved regular file for reading. Refuses directories, FIFOs, sockets, devices and
/// a final component that is (or became) a symlink.
pub fn open_regular(r: &Resolved) -> Result<File, OpenError> {
    let before = check_final(&r.abs)?;
    let file = File::open(&r.abs).map_err(io_error)?;
    same_file(&file, &before)?;
    Ok(file)
}

/// Writes `bytes` to a resolved path, creating it and its parent folders. A new file is
/// created exclusively (a symlink that appeared there makes it fail); an existing one must be
/// a regular file and is checked to be that same file before it is truncated.
pub fn write_regular(r: &Resolved, bytes: &[u8]) -> Result<(), OpenError> {
    if let Some(parent) = r.abs.parent() {
        std::fs::create_dir_all(parent).map_err(OpenError::Io)?;
        let real = std::fs::canonicalize(parent).map_err(OpenError::Io)?;
        if !real.starts_with(&r.root) {
            return Err(OpenError::Changed);
        }
    }
    let mut file = match std::fs::symlink_metadata(&r.abs) {
        Err(e) if e.kind() == ErrorKind::NotFound => match OpenOptions::new().write(true).create_new(true).open(&r.abs) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => return Err(OpenError::Changed),
            Err(e) => return Err(OpenError::Io(e)),
        },
        Err(e) => return Err(io_error(e)),
        Ok(_) => {
            let before = check_final(&r.abs)?;
            let file = OpenOptions::new().write(true).open(&r.abs).map_err(io_error)?;
            same_file(&file, &before)?;
            file.set_len(0).map_err(OpenError::Io)?;
            file
        }
    };
    file.write_all(bytes).map_err(OpenError::Io)
}
