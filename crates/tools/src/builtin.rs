//! The built-in tools by access level, and the grants that trim them.

use std::sync::Arc;

use model::{ToolAccess, ToolGrants};

use crate::api::ToolRegistry;
use crate::{Bash, EditFile, Grep, ListDir, ReadFile, WriteFile};

/// Names of the built-in tools, the one place they are spelled out.
pub mod names {
    pub const READ_FILE: &str = "read_file";
    pub const LIST_DIR: &str = "list_dir";
    pub const GREP: &str = "grep";
    pub const EDIT_FILE: &str = "edit_file";
    pub const WRITE_FILE: &str = "write_file";
    pub const BASH: &str = "bash";
}

/// The built-in tools an access level gets: `ReadOnly` the three read tools, `ReadAndRun` also
/// `bash`, `Full` also `edit_file` and `write_file`.
pub fn builtin(access: ToolAccess) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadFile));
    registry.register(Arc::new(ListDir));
    registry.register(Arc::new(Grep));
    if matches!(access, ToolAccess::ReadAndRun | ToolAccess::Full) {
        registry.register(Arc::new(Bash));
    }
    if access == ToolAccess::Full {
        registry.register(Arc::new(EditFile));
        registry.register(Arc::new(WriteFile));
    }
    registry
}

/// Removes the tools `grants.deny` names. `grants.allow` names tools other crates register
/// (MCP tools, session tools); adding those is the caller's job.
pub fn apply_grants(registry: &mut ToolRegistry, grants: &ToolGrants) {
    for name in &grants.deny {
        registry.remove(name);
    }
}
