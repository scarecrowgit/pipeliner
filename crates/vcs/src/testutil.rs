//! Helpers for tests: throwaway repositories made with the real `git`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// A temporary folder that holds the repositories of one test.
pub struct Sandbox {
    dir: TempDir,
    root: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical temp dir");
        Self { dir, root }
    }

    pub fn path(&self) -> &Path {
        let _ = &self.dir;
        &self.root
    }

    /// A repository on `main` with one commit (`README.md`) and a fixed local identity.
    pub fn repo(&self, name: &str) -> PathBuf {
        let repo = self.root.join(name);
        std::fs::create_dir_all(&repo).expect("create repo dir");
        git(&repo, &["init", "-q", "-b", "main"]);
        configure(&repo);
        write(&repo, "README.md", "hello\n");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    /// An empty bare repository, for use as a remote.
    pub fn bare(&self, name: &str) -> PathBuf {
        let repo = self.root.join(name);
        std::fs::create_dir_all(&repo).expect("create bare dir");
        git(&repo, &["init", "-q", "--bare", "-b", "main"]);
        repo
    }
}

/// Sets the identity and turns signing off in the repository's own config.
pub fn configure(repo: &Path) {
    git(repo, &["config", "user.name", "Test Author"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "commit.gpgsign", "false"]);
}

/// Runs git with an isolated global config; panics when it fails; returns stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(Stdio::null())
        .output()
        .expect("start git");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Writes `content` to `rel` under `dir`, creating folders.
pub fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dirs");
    }
    std::fs::write(path, content).expect("write file");
}

/// Writes a file and commits it.
pub fn commit_file(repo: &Path, rel: &str, content: &str) {
    write(repo, rel, content);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", &format!("add {rel}")]);
}
