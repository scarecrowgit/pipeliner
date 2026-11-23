//! Content-addressed files: images, renders, long tool outputs.
//!
//! A blob's id is the SHA-256 of its bytes in lowercase hex; it lives at
//! `blobs/<first two>/<next two>/<id>`. Writes go to a temp file in the same directory and are
//! renamed into place, so a crash never leaves a partial blob under a valid name.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use model::BlobId;
use sha2::{Digest as _, Sha256};

use crate::Store;

pub(crate) struct Blobs {
    dir: PathBuf,
}

impl Blobs {
    pub(crate) fn new(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn path(&self, id: &BlobId) -> Result<PathBuf> {
        let hex = id.as_str();
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            bail!("{hex:?} is not a blob id");
        }
        Ok(self.dir.join(&hex[..2]).join(&hex[2..4]).join(hex))
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

impl Store {
    /// Stores bytes and returns their id. Storing the same bytes again is a no-op.
    pub fn put_blob(&self, bytes: &[u8]) -> Result<BlobId> {
        let id = BlobId(hex(&Sha256::digest(bytes)));
        let path = self.blobs.path(&id)?;
        if path.is_file() {
            return Ok(id);
        }
        let dir = path.parent().context("a blob path has a parent")?;
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let temp = dir.join(format!(".{}.{}.tmp", id.as_str(), base::id::new_id()));
        let written = (|| -> Result<()> {
            let mut file = fs::File::create(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temp, &path)?;
            Ok(())
        })();
        if let Err(err) = written {
            let _ = fs::remove_file(&temp);
            return Err(err.context(format!("write blob {}", path.display())));
        }
        Ok(id)
    }

    /// A blob's bytes.
    pub fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>> {
        let path = self.blobs.path(id)?;
        fs::read(&path).with_context(|| format!("read blob {}", path.display()))
    }

    /// Where a blob's file is (or would be), for handing to an image loader.
    pub fn blob_path(&self, id: &BlobId) -> Result<PathBuf> {
        self.blobs.path(id)
    }

    /// Whether a blob is stored.
    pub fn blob_exists(&self, id: &BlobId) -> Result<bool> {
        Ok(self.blobs.path(id)?.is_file())
    }
}
