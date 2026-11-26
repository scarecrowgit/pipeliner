//! Where things live on disk.

use std::path::PathBuf;

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.config/pipeliner` (or `$PIPELINER_CONFIG_DIR`).
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PIPELINER_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    dirs::config_dir()
        .unwrap_or_else(|| home().join(".config"))
        .join("pipeliner")
}

/// `~/.local/share/pipeliner` (or `$PIPELINER_DATA_DIR`): database, worktrees, renders.
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PIPELINER_DATA_DIR") {
        return PathBuf::from(p);
    }
    dirs::data_dir()
        .unwrap_or_else(|| home().join(".local/share"))
        .join("pipeliner")
}

pub fn worktrees_dir() -> PathBuf {
    data_dir().join("worktrees")
}

pub fn renders_dir() -> PathBuf {
    data_dir().join("renders")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Images attached to chat messages.
pub fn attachments_dir() -> PathBuf {
    data_dir().join("attachments")
}
