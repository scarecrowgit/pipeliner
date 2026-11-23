//! Render pictures of old runs, copied into the blob store.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use model::{BlobId, Render};
use serde::Deserialize;

use super::Ctx;
use super::report::Kind;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A render as the old run recorded it.
#[derive(Deserialize)]
pub(super) struct OldRender {
    /// File stem of the rendered file: `issue-board`.
    pub(super) screen: String,
    /// The picture before the change; none for a new file.
    #[serde(default)]
    pub(super) before: Option<PathBuf>,
    pub(super) after: PathBuf,
}

/// Where a picture is: the recorded path when it lies in the old data directory, else the
/// run's own `renders` folder under the same file name (the data directory may have moved).
fn find(cx: &Ctx, run: &str, recorded: &Path) -> Option<PathBuf> {
    let run_dir = cx.data.join("runs").join(run);
    let candidate = if recorded.is_absolute() {
        recorded.to_path_buf()
    } else {
        run_dir.join(recorded)
    };
    let inside = candidate.starts_with(&cx.data) && !candidate.components().any(|c| matches!(c, Component::ParentDir));
    if inside && candidate.is_file() {
        return Some(candidate);
    }
    let fallback = run_dir.join("renders").join(recorded.file_name()?);
    fallback.is_file().then_some(fallback)
}

/// Width and height from a PNG header; zeros for anything else.
fn png_size(bytes: &[u8]) -> (u32, u32) {
    if bytes.len() < 24 || bytes[..8] != PNG_SIGNATURE {
        return (0, 0);
    }
    let be = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    (be(16), be(20))
}

struct Picture {
    blob: BlobId,
    size: (u32, u32),
}

/// Reads one picture and, unless this is a dry run, stores it.
fn load(cx: &Ctx, run: &str, recorded: &Path) -> Result<Picture> {
    let Some(path) = find(cx, run, recorded) else {
        bail!("the picture {} is not there", recorded.display());
    };
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let blob = if cx.dry {
        BlobId::default()
    } else {
        cx.store.put_blob(&bytes)?
    };
    Ok(Picture {
        blob,
        size: png_size(&bytes),
    })
}

/// The renders of a run with their pictures in the blob store, and how many pictures were
/// copied. A picture that cannot be read is reported; a render without its "after" is left
/// out, one without its "before" is kept as a new file.
pub(super) fn copy(cx: &mut Ctx, run: &str, old: &[OldRender]) -> (Vec<Render>, usize) {
    let mut renders = Vec::new();
    let mut copied = 0;
    for render in old {
        let item = format!("run {run}, {}", render.screen);
        let before = render.before.as_deref().and_then(|path| match load(cx, run, path) {
            Ok(picture) => {
                copied += 1;
                Some(picture)
            }
            Err(err) => {
                cx.report
                    .fail(Kind::Image, format!("{item} (before)"), format!("{err:#}"));
                None
            }
        });
        let after = match load(cx, run, &render.after) {
            Ok(picture) => {
                copied += 1;
                picture
            }
            Err(err) => {
                cx.report
                    .fail(Kind::Image, format!("{item} (after)"), format!("{err:#}"));
                continue;
            }
        };
        let path = if render.screen.contains(['.', '/']) {
            render.screen.clone()
        } else {
            format!("screens/{}.html", render.screen)
        };
        renders.push(Render {
            path,
            before: before.map(|p| p.blob),
            after: after.blob,
            width: after.size.0,
            height: after.size.1,
        });
    }
    (renders, copied)
}
