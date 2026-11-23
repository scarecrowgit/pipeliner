//! Where the app keeps its files.
//!
//! One [`Paths`] value is resolved at start-up and handed down; nothing else reads `$HOME` or
//! the XDG variables. Tests build one with [`Paths::under`] inside a temp directory.

use std::path::{Component, Path, PathBuf};

/// The app's directories: the user's home, the config directory and file, and the data
/// directory. Everything else is derived from these four.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// The user's home directory (`$HOME`).
    pub home: PathBuf,
    /// Directory of user-edited files: the config file, the agent library, skills.
    pub config_dir: PathBuf,
    /// The provider/config file (`config.toml` unless `$HARNESS_CONFIG` says otherwise).
    pub config_file: PathBuf,
    /// Directory of app-written state. The old app's files live directly in it; the new app
    /// writes only under [`Paths::v2_dir`].
    pub data_dir: PathBuf,
}

impl Paths {
    /// Paths for this process, from `$HOME`, `$HARNESS_CONFIG`, `$HARNESS_DATA` and the XDG
    /// variables. See [`Paths::resolve`] for the rules.
    pub fn from_env() -> Self {
        Self::resolve(|name| std::env::var(name).ok())
    }

    /// The rules behind [`Paths::from_env`], with the environment passed in as a lookup so
    /// they can be tested without touching the process environment.
    ///
    /// - `home` is `$HOME` (`/` when unset).
    /// - `config_file` is `$HARNESS_CONFIG` when set and non-empty, else
    ///   `config_dir/config.toml`.
    /// - `config_dir` is the parent of `$HARNESS_CONFIG` when that is set, else
    ///   `$XDG_CONFIG_HOME/harness` (only for an absolute `$XDG_CONFIG_HOME`), else
    ///   `~/.config/harness`.
    /// - `data_dir` is `$HARNESS_DATA` when set and non-empty, else `$XDG_DATA_HOME/harness`
    ///   (absolute only), else `~/.local/share/harness`.
    pub fn resolve(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let get = |name: &str| lookup(name).filter(|v| !v.is_empty());
        let absolute = |name: &str| get(name).map(PathBuf::from).filter(|p| p.is_absolute());

        let home = get("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let (config_dir, config_file) = match get("HARNESS_CONFIG").map(PathBuf::from) {
            Some(file) => {
                let dir = match file.parent() {
                    Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
                    _ => PathBuf::from("."),
                };
                (dir, file)
            }
            None => {
                let dir = absolute("XDG_CONFIG_HOME")
                    .unwrap_or_else(|| home.join(".config"))
                    .join("harness");
                let file = dir.join("config.toml");
                (dir, file)
            }
        };
        let data_dir = get("HARNESS_DATA").map(PathBuf::from).unwrap_or_else(|| {
            absolute("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local/share"))
                .join("harness")
        });
        Self { home, config_dir, config_file, data_dir }
    }

    /// Self-contained paths for tests: `root/home`, `root/config` (with `config.toml`) and
    /// `root/data`. Nothing is created on disk.
    pub fn under(root: &Path) -> Self {
        let config_dir = root.join("config");
        Self {
            home: root.join("home"),
            config_file: config_dir.join("config.toml"),
            config_dir,
            data_dir: root.join("data"),
        }
    }

    /// The user's agent library (agent Markdown files).
    pub fn library_dir(&self) -> PathBuf {
        self.config_dir.join("library")
    }

    /// The user's skills (one directory per skill).
    pub fn skills_dir(&self) -> PathBuf {
        self.config_dir.join("skills")
    }

    /// Pipeline files of the old app. Read-only, for import.
    pub fn v1_pipelines_dir(&self) -> PathBuf {
        self.config_dir.join("pipelines")
    }

    /// State of the old app (`sessions/`, `issues.json`, `runs/`, `inbox.json`). Read-only,
    /// for import.
    pub fn v1_data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    /// Root of everything the new app writes.
    pub fn v2_dir(&self) -> PathBuf {
        self.data_dir.join("v2")
    }

    /// The app database.
    pub fn db_file(&self) -> PathBuf {
        self.v2_dir().join("harness.db")
    }

    /// Large payloads kept outside the database (attachments, tool outputs).
    pub fn blobs_dir(&self) -> PathBuf {
        self.v2_dir().join("blobs")
    }

    /// Log files.
    pub fn log_dir(&self) -> PathBuf {
        self.v2_dir().join("log")
    }

    /// Git worktrees created for chats and issue runs.
    pub fn worktrees_dir(&self) -> PathBuf {
        self.home.join(".harness/worktrees")
    }

    /// A path as the user typed it: `~` and `~/x` resolve under the home directory, anything
    /// else (including `~user/x`) is taken as written.
    pub fn expand(&self, text: &str) -> PathBuf {
        match text.strip_prefix('~') {
            Some("") => self.home.clone(),
            Some(rest) if rest.starts_with('/') => {
                let rest = rest.trim_start_matches('/');
                if rest.is_empty() { self.home.clone() } else { self.home.join(rest) }
            }
            _ => PathBuf::from(text),
        }
    }

    /// A path for showing to the user: under the home directory it reads `~/x`, the home
    /// directory itself `~`, anything else unchanged.
    pub fn display(&self, path: &Path) -> String {
        // A home of "/" (unset $HOME) would turn every absolute path into "~/…".
        let real_home = self.home.components().any(|c| matches!(c, Component::Normal(_)));
        match path.strip_prefix(&self.home) {
            Ok(rest) if real_home && rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) if real_home => format!("~/{}", rest.display()),
            _ => path.display().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolve(vars: &[(&str, &str)]) -> Paths {
        let map: HashMap<String, String> =
            vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Paths::resolve(|name| map.get(name).cloned())
    }

    #[test]
    fn defaults_follow_home() {
        let p = resolve(&[("HOME", "/home/u")]);
        assert_eq!(p.home, PathBuf::from("/home/u"));
        assert_eq!(p.config_dir, PathBuf::from("/home/u/.config/harness"));
        assert_eq!(p.config_file, PathBuf::from("/home/u/.config/harness/config.toml"));
        assert_eq!(p.data_dir, PathBuf::from("/home/u/.local/share/harness"));
    }

    #[test]
    fn xdg_variables_apply_only_when_absolute() {
        let p = resolve(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_DATA_HOME", "/dat"),
        ]);
        assert_eq!(p.config_dir, PathBuf::from("/cfg/harness"));
        assert_eq!(p.config_file, PathBuf::from("/cfg/harness/config.toml"));
        assert_eq!(p.data_dir, PathBuf::from("/dat/harness"));

        let p = resolve(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "relative/cfg"),
            ("XDG_DATA_HOME", ""),
        ]);
        assert_eq!(p.config_dir, PathBuf::from("/home/u/.config/harness"));
        assert_eq!(p.data_dir, PathBuf::from("/home/u/.local/share/harness"));
    }

    #[test]
    fn harness_config_names_the_file_and_its_directory() {
        let p = resolve(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("HARNESS_CONFIG", "/etc/h/alt.toml"),
        ]);
        assert_eq!(p.config_file, PathBuf::from("/etc/h/alt.toml"));
        assert_eq!(p.config_dir, PathBuf::from("/etc/h"));
        assert_eq!(p.library_dir(), PathBuf::from("/etc/h/library"));

        let bare = resolve(&[("HOME", "/home/u"), ("HARNESS_CONFIG", "alt.toml")]);
        assert_eq!(bare.config_file, PathBuf::from("alt.toml"));
        assert_eq!(bare.config_dir, PathBuf::from("."));

        let empty = resolve(&[("HOME", "/home/u"), ("HARNESS_CONFIG", "")]);
        assert_eq!(empty.config_file, PathBuf::from("/home/u/.config/harness/config.toml"));
    }

    #[test]
    fn harness_data_wins_over_xdg() {
        let p = resolve(&[
            ("HOME", "/home/u"),
            ("XDG_DATA_HOME", "/dat"),
            ("HARNESS_DATA", "/srv/harness"),
        ]);
        assert_eq!(p.data_dir, PathBuf::from("/srv/harness"));
        let p = resolve(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/dat"), ("HARNESS_DATA", "")]);
        assert_eq!(p.data_dir, PathBuf::from("/dat/harness"));
    }

    #[test]
    fn missing_home_falls_back_to_root() {
        let p = resolve(&[]);
        assert_eq!(p.home, PathBuf::from("/"));
        assert_eq!(p.config_file, PathBuf::from("/.config/harness/config.toml"));
        assert_eq!(p.display(Path::new("/etc/hosts")), "/etc/hosts");
    }

    #[test]
    fn derived_directories() {
        let p = Paths::under(Path::new("/t"));
        assert_eq!(p.home, PathBuf::from("/t/home"));
        assert_eq!(p.config_dir, PathBuf::from("/t/config"));
        assert_eq!(p.config_file, PathBuf::from("/t/config/config.toml"));
        assert_eq!(p.data_dir, PathBuf::from("/t/data"));
        assert_eq!(p.library_dir(), PathBuf::from("/t/config/library"));
        assert_eq!(p.skills_dir(), PathBuf::from("/t/config/skills"));
        assert_eq!(p.v1_pipelines_dir(), PathBuf::from("/t/config/pipelines"));
        assert_eq!(p.v1_data_dir(), PathBuf::from("/t/data"));
        assert_eq!(p.v2_dir(), PathBuf::from("/t/data/v2"));
        assert_eq!(p.db_file(), PathBuf::from("/t/data/v2/harness.db"));
        assert_eq!(p.blobs_dir(), PathBuf::from("/t/data/v2/blobs"));
        assert_eq!(p.log_dir(), PathBuf::from("/t/data/v2/log"));
        assert_eq!(p.worktrees_dir(), PathBuf::from("/t/home/.harness/worktrees"));
    }

    #[test]
    fn under_a_temp_dir_stays_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::under(dir.path());
        for path in [p.db_file(), p.worktrees_dir(), p.library_dir(), p.config_file.clone()] {
            assert!(path.starts_with(dir.path()), "{path:?}");
        }
    }

    #[test]
    fn expand_resolves_only_the_users_own_tilde() {
        let p = Paths::under(Path::new("/t"));
        assert_eq!(p.expand("~"), PathBuf::from("/t/home"));
        assert_eq!(p.expand("~/"), PathBuf::from("/t/home"));
        assert_eq!(p.expand("~/src/app"), PathBuf::from("/t/home/src/app"));
        assert_eq!(p.expand("~//src"), PathBuf::from("/t/home/src"));
        assert_eq!(p.expand("~other/x"), PathBuf::from("~other/x"));
        assert_eq!(p.expand("/abs/~/x"), PathBuf::from("/abs/~/x"));
        assert_eq!(p.expand("rel/x"), PathBuf::from("rel/x"));
        assert_eq!(p.expand(""), PathBuf::from(""));
    }

    #[test]
    fn display_shortens_paths_under_home() {
        let p = Paths::under(Path::new("/t"));
        assert_eq!(p.display(Path::new("/t/home")), "~");
        assert_eq!(p.display(Path::new("/t/home/src/app")), "~/src/app");
        assert_eq!(p.display(Path::new("/t/homework/x")), "/t/homework/x");
        assert_eq!(p.display(Path::new("/etc/hosts")), "/etc/hosts");
        assert_eq!(p.display(Path::new("rel")), "rel");
        assert_eq!(p.display(&p.expand("~/a b")), "~/a b");
    }
}
