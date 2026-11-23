//! Tests for the tools crate. Every test works in its own temp directory.

mod bash;
mod diff;
mod edit;
mod misc;
mod permission;
mod read;
mod sandbox;
mod security;
mod shell;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base::cancel::CancelToken;
use serde_json::Value;
use tempfile::TempDir;

use crate::api::{Tool, ToolContext, ToolOutput};

/// A temp workdir with an environment and a cancel token.
pub(crate) struct Fixture {
    pub dir: TempDir,
    pub env: BTreeMap<String, String>,
    pub cancel: CancelToken,
}

impl Fixture {
    pub fn new() -> Self {
        Self { dir: TempDir::new().expect("temp dir"), env: BTreeMap::new(), cancel: CancelToken::new() }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    pub fn ctx(&self) -> ToolContext<'_> {
        ToolContext { root: self.dir.path(), env: &self.env, cancel: &self.cancel }
    }

    /// Writes `text` to `rel`, creating parent folders.
    pub fn write(&self, rel: &str, text: &str) {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).expect("read")
    }

    pub fn run(&self, tool: &dyn Tool, input: Value) -> ToolOutput {
        tool.run(&self.ctx(), &input)
    }
}

/// Inputs no tool may panic on.
pub(crate) fn junk_inputs() -> Vec<Value> {
    use serde_json::json;
    vec![
        Value::Null,
        json!(42),
        json!("not json"),
        json!("{\"path\": 5}"),
        json!([1, 2]),
        json!({}),
        json!({"path": 5, "command": [], "pattern": {}, "old_string": 1, "content": null}),
        json!({"path": "", "command": "", "pattern": "", "old_string": "", "new_string": "", "content": ""}),
        json!({"path": "../../etc/passwd", "pattern": "(", "glob": "[", "offset": -1, "limit": "x"}),
        json!({"path": "a", "offset": 1.5, "limit": 1e300, "replace_all": "maybe", "timeout_secs": -3}),
    ]
}
