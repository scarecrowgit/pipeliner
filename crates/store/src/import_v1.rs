//! One-time import of the old app's files into the store.
//!
//! The old app kept chat sessions as `sessions/<id>.json`, issues in `issues.json`, each issue's
//! latest pipeline run in `runs/<id>.json` (with render pictures beside it), Inbox reports in
//! `inbox.json` and the pinned/archived chats in `chat_flags.json`, all directly in the data
//! directory. [`import_v1`] reads them and writes the new store; it never writes, renames or
//! deletes anything in the old directory, so the old app keeps working next to the new one.
//!
//! The import can be repeated: a session is found again by its legacy id, an issue by its key,
//! a run, an Inbox item by its id. Running it twice imports nothing the second time, and
//! running it after the old app saved something new imports just that. One bad file or item is
//! counted and named in the [`ImportReport`]; it never stops the rest.
//!
//! | Module | Imports |
//! | --- | --- |
//! | `sessions` | Chats: messages, usage, workdir, pinned and archived flags |
//! | `text` | The old inline encodings of attachments, `!shell` runs and dividers |
//! | `issues` | Issues, comments, the project, the key counter |
//! | `runs`, `renders` | Pipeline runs as history, with their logs and render pictures |
//! | `inbox` | Inbox reports |

mod inbox;
mod issues;
mod renders;
mod report;
mod runs;
mod sessions;
mod text;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use base::paths::Paths;
use model::{Millis, SessionId};

pub use report::{ImportReport, Kind, Problem, Tally};

use crate::Store;

/// The marker set once an import ran for real, for the app to know the old data was offered.
pub const IMPORT_MARKER: &str = "v1_import_done";

/// How an import runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// Parse everything and return the report a real run would give, without writing to the
    /// store.
    pub dry_run: bool,
}

/// Imports the old app's data under `paths.v1_data_dir()` into `store`. See the module docs.
///
/// Fails only when the import cannot even be started or finished; problems with single files
/// are in the returned report.
pub fn import_v1(store: &Store, paths: &Paths, options: &ImportOptions) -> Result<ImportReport> {
    let mut cx = Ctx {
        store,
        paths,
        data: paths.v1_data_dir(),
        dry: options.dry_run,
        report: ImportReport {
            dry_run: options.dry_run,
            ..ImportReport::default()
        },
        sessions: HashMap::new(),
    };
    sessions::import(&mut cx);
    issues::import(&mut cx);
    inbox::import(&mut cx);
    if !cx.dry {
        store.set_marker(IMPORT_MARKER, &base::time::now_ms().to_string())?;
    }
    Ok(cx.report)
}

/// What every step of the import shares.
pub(crate) struct Ctx<'a> {
    pub(crate) store: &'a Store,
    pub(crate) paths: &'a Paths,
    /// The old app's data directory, read only.
    pub(crate) data: PathBuf,
    pub(crate) dry: bool,
    pub(crate) report: ImportReport,
    /// Old session id to the session it became (or would become in a dry run), for the Inbox.
    pub(crate) sessions: HashMap<String, SessionId>,
}

/// The `.json` files of a directory, sorted by name; none when the directory does not exist.
pub(crate) fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("read {}", dir.display())),
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            name.ends_with(".json") && !name.starts_with('.') && path.is_file()
        })
        .collect();
    files.sort();
    Ok(files)
}

/// A file read and parsed as JSON, with errors that name it.
pub(crate) fn read_json(path: &Path) -> Result<serde_json::Value> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// A file's name relative to the data directory, for reports: `sessions/1-2.json`.
pub(crate) fn rel(data: &Path, path: &Path) -> String {
    path.strip_prefix(data).unwrap_or(path).display().to_string()
}

/// The old app stored unix seconds; a value that is already milliseconds is kept.
pub(crate) fn secs_to_ms(value: u64) -> Millis {
    let value = i64::try_from(value).unwrap_or(i64::MAX);
    if value >= 100_000_000_000 {
        value
    } else {
        value.saturating_mul(1000)
    }
}

/// A file's modification time in milliseconds, when the filesystem has one.
pub(crate) fn modified_ms(path: &Path) -> Option<Millis> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Millis::try_from(since_epoch.as_millis()).ok()
}
