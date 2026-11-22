//! Working folders for chat sessions: git facts, worktrees, recent folders, folder scan,
//! worktree creation and cloning. gpui-free and blocking (run on background threads).
//! Git data comes from the `git` CLI with short timeouts; every function degrades to
//! `None`/empty instead of failing when git is missing or the folder isn't a repo.

use std::collections::BTreeSet;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provider {
    GitHub,
    GitLab,
    Bitbucket,
    /// Any other host (shown by name).
    Other(String),
}

impl Provider {
    pub fn label(&self) -> &str {
        match self {
            Provider::GitHub => "GitHub",
            Provider::GitLab => "GitLab",
            Provider::Bitbucket => "Bitbucket",
            Provider::Other(h) => h,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitInfo {
    /// `git rev-parse --show-toplevel` of the folder (the worktree root).
    pub root: PathBuf,
    /// Root of the main worktree (differs from `root` inside a linked worktree).
    pub main_root: PathBuf,
    pub is_linked_worktree: bool,
    /// Current branch; `None` when detached.
    pub branch: Option<String>,
    /// Default branch guess: `origin/HEAD` target, else `main`/`master` if it exists.
    pub default_branch: Option<String>,
    pub remote_url: Option<String>,
    pub provider: Option<Provider>,
    /// `host/owner/repo` from the remote (e.g. `github.com/acme/payments-api`).
    pub remote_path: Option<String>,
    /// Number of changed (staged + unstaged + untracked) files.
    pub changed: usize,
    /// Commits ahead/behind the upstream (0 when there's no upstream).
    pub ahead: usize,
    pub behind: usize,
}

impl GitInfo {
    /// Short human state: "clean on main", "3 changed on feat/x", "2 uncommitted files ·
    /// 1 ahead of main" style used by the UI.
    pub fn summary(&self) -> String {
        let mut out = match (&self.branch, self.changed) {
            (None, 0) => "detached HEAD".to_string(),
            (None, n) => format!("{n} changed · detached HEAD"),
            (Some(b), 0) => format!("clean on {b}"),
            (Some(b), n) => format!("{n} changed on {b}"),
        };
        if self.ahead > 0 {
            out.push_str(&format!(" · {} ahead", self.ahead));
        }
        if self.behind > 0 {
            out.push_str(&format!(" · {} behind", self.behind));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_main: bool,
}

/// What the agent will find in a folder (the picker's "Found in this folder").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderScan {
    /// `("AGENTS.md" | "CLAUDE.md", line count)` — the first that exists.
    pub instructions: Option<(String, usize)>,
    /// Skill names under `.harness/skills` and `.claude/skills`.
    pub skills: Vec<String>,
    /// Command names under `.harness/commands` and `.claude/commands` (file stems).
    pub commands: Vec<String>,
    /// `.mcp.json` server names.
    pub mcp_servers: Vec<String>,
    /// `GitInfo::summary` when it is a repo.
    pub git_status: Option<String>,
}

// ---------------------------------------------------------------------------------------
// Process runner
// ---------------------------------------------------------------------------------------

/// Timeout for read-only git queries.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Timeout for `git worktree add` (checks out files).
const WORKTREE_TIMEOUT: Duration = Duration::from_secs(120);
/// Timeout for clones.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);
/// Max stdout kept from a git query (e.g. `status` of a huge dirty tree).
const STDOUT_CAP: usize = 1024 * 1024;
const STDERR_CAP: usize = 16 * 1024;
const POLL: Duration = Duration::from_millis(10);

struct RunOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn spawn_reader<R: Read + Send + 'static>(mut src: R, cap: usize) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match src.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = tx.send(buf);
    });
    rx
}

fn kill_group(pgid: u32) {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -KILL -{pgid} 2>/dev/null"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Run `cmd` with null stdin, captured (capped) output and a timeout. `Err` when the
/// program can't be started or timed out (the whole process group is killed).
fn run(mut cmd: Command, timeout: Duration) -> Result<RunOutput, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd.spawn().map_err(|e| format!("failed to start {program}: {e}"))?;
    let pgid = child.id();
    let out_rx = child.stdout.take().map(|s| spawn_reader(s, STDOUT_CAP));
    let err_rx = child.stderr.take().map(|s| spawn_reader(s, STDERR_CAP));

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if start.elapsed() >= timeout {
            kill_group(pgid);
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} timed out after {} s", timeout.as_secs()));
        }
        std::thread::sleep(POLL);
    };
    let collect = |rx: Option<mpsc::Receiver<Vec<u8>>>| -> String {
        rx.and_then(|rx| rx.recv_timeout(Duration::from_secs(1)).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    let stdout = collect(out_rx);
    let stderr = collect(err_rx);
    Ok(RunOutput { success: status.is_some_and(|s| s.success()), stdout, stderr })
}

fn non_interactive(cmd: &mut Command) {
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_OPTIONAL_LOCKS", "0");
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"] {
        cmd.env_remove(var);
    }
}

fn git_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    non_interactive(&mut cmd);
    cmd
}

/// Stdout of a successful read-only git query, `None` otherwise.
fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = git_cmd(dir);
    cmd.args(args);
    run(cmd, READ_TIMEOUT).ok().filter(|o| o.success).map(|o| o.stdout)
}

fn first_line(s: &str) -> Option<String> {
    s.lines().next().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string)
}

/// Removes `user[:password]@` after `scheme://` anywhere in `s`.
fn redact_credentials(s: &str) -> String {
    let re = regex::Regex::new(r"([A-Za-z][A-Za-z0-9+.\-]*://)[^/\s@]+@").expect("valid regex");
    re.replace_all(s, "$1").into_owned()
}

fn command_error(what: &str, out: &RunOutput) -> anyhow::Error {
    let msg = out.stderr.trim();
    let msg = if msg.is_empty() { out.stdout.trim() } else { msg };
    let msg = redact_credentials(msg);
    if msg.is_empty() { anyhow!("{what} failed") } else { anyhow!("{what} failed: {msg}") }
}

// ---------------------------------------------------------------------------------------
// Git queries
// ---------------------------------------------------------------------------------------

fn absolutize(base: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    let abs = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    abs.canonicalize().unwrap_or(abs)
}

/// Parsed `git status --porcelain=v2 --branch`.
#[derive(Debug, Default, PartialEq, Eq)]
struct StatusFacts {
    /// `Some(None)` = detached, `None` = no header seen.
    head: Option<Option<String>>,
    changed: usize,
    ahead: usize,
    behind: usize,
}

fn parse_status_v2(out: &str) -> StatusFacts {
    let mut facts = StatusFacts::default();
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            let h = h.trim();
            facts.head = Some((h != "(detached)").then(|| h.to_string()));
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(a) = part.strip_prefix('+') {
                    facts.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = part.strip_prefix('-') {
                    facts.behind = b.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.starts_with('!') && !line.trim().is_empty() {
            facts.changed += 1;
        }
    }
    facts
}

/// Default branch from `for-each-ref --format=%(refname)\t%(symref)` over
/// `refs/remotes/origin/HEAD refs/heads/main refs/heads/master`.
fn parse_default_branch(out: &str) -> Option<String> {
    let mut has_main = false;
    let mut has_master = false;
    for line in out.lines() {
        let (name, symref) = line.split_once('\t').unwrap_or((line, ""));
        match name.trim() {
            "refs/remotes/origin/HEAD" => {
                if let Some(b) = symref.trim().strip_prefix("refs/remotes/origin/")
                    && !b.is_empty()
                {
                    return Some(b.to_string());
                }
            }
            "refs/heads/main" => has_main = true,
            "refs/heads/master" => has_master = true,
            _ => {}
        }
    }
    if has_main {
        Some("main".into())
    } else if has_master {
        Some("master".into())
    } else {
        None
    }
}

/// `(provider, host/owner/repo)` from a remote URL (https, ssh://, git://, scp-style).
/// `None` for local paths / `file://` and unparsable URLs.
pub(crate) fn parse_remote(url: &str) -> Option<(Provider, String)> {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        // Strip a port (but keep bracketed IPv6 intact).
        let host = if host.starts_with('[') {
            host.split_once(']').map_or(host, |(h, _)| h.trim_start_matches('['))
        } else {
            host.split_once(':').map_or(host, |(h, _)| h)
        };
        (host, path)
    } else {
        // scp-style `[user@]host:path`; the part before ':' must not contain '/'.
        let (before, path) = url.split_once(':')?;
        if before.contains('/') || before.is_empty() {
            return None;
        }
        let host = before.rsplit_once('@').map_or(before, |(_, h)| h);
        (host, path)
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/');
    let path = path.strip_prefix('~').unwrap_or(path).trim_start_matches('/');
    if path.is_empty() {
        return None;
    }
    let provider = if host == "github.com" || host == "www.github.com" {
        Provider::GitHub
    } else if host.starts_with("gitlab.") {
        Provider::GitLab
    } else if host == "bitbucket.org" || host == "www.bitbucket.org" {
        Provider::Bitbucket
    } else {
        Provider::Other(host.clone())
    };
    Some((provider, format!("{host}/{path}")))
}

/// Facts about the repository containing `dir`, or `None` if it isn't one.
pub fn git_info(dir: &Path) -> Option<GitInfo> {
    let rp = git_out(dir, &["rev-parse", "--show-toplevel", "--git-dir", "--git-common-dir"])?;
    let mut lines = rp.lines().map(str::trim);
    let root = absolutize(dir, lines.next().filter(|l| !l.is_empty())?);
    let git_dir = absolutize(dir, lines.next()?);
    let common_dir = absolutize(dir, lines.next()?);
    let is_linked_worktree = git_dir != common_dir;
    let main_root = if !is_linked_worktree {
        root.clone()
    } else {
        let from_list = worktrees(dir).into_iter().find(|w| w.is_main).map(|w| w.path);
        let from_common = match common_dir.file_name() {
            Some(n) if n == ".git" => common_dir.parent().map(Path::to_path_buf),
            _ => None,
        };
        from_list.or(from_common).unwrap_or_else(|| root.clone())
    };

    let status = git_out(dir, &["status", "--porcelain=v2", "--branch", "-uall"])
        .map(|s| parse_status_v2(&s))
        .unwrap_or_default();
    let branch = match status.head.clone() {
        Some(head) => head,
        None => git_out(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).and_then(|s| first_line(&s)),
    };

    let remote_url = git_out(dir, &["remote", "get-url", "origin"])
        .and_then(|s| first_line(&s))
        .map(|u| redact_credentials(&u));
    let (provider, remote_path) = match remote_url.as_deref().and_then(parse_remote) {
        Some((p, path)) => (Some(p), Some(path)),
        None => (None, None),
    };

    let default_branch = git_out(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)\t%(symref)",
            "refs/remotes/origin/HEAD",
            "refs/heads/main",
            "refs/heads/master",
        ],
    )
    .and_then(|s| parse_default_branch(&s));

    Some(GitInfo {
        root,
        main_root,
        is_linked_worktree,
        branch,
        default_branch,
        remote_url,
        provider,
        remote_path,
        changed: status.changed,
        ahead: status.ahead,
        behind: status.behind,
    })
}

fn parse_worktrees(out: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut index = 0;
    for block in out.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        let mut bare = false;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(r) = line.strip_prefix("branch ") {
                branch = Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
            } else if line == "bare" {
                bare = true;
            }
        }
        let Some(path) = path else { continue };
        // git prints the main worktree first; a bare main repo has no worktree to show.
        if !bare {
            list.push(Worktree { path, branch, is_main: index == 0 });
        }
        index += 1;
    }
    list
}

/// `git worktree list --porcelain` of the repo containing `dir` (main first).
pub fn worktrees(dir: &Path) -> Vec<Worktree> {
    git_out(dir, &["worktree", "list", "--porcelain"]).map(|s| parse_worktrees(&s)).unwrap_or_default()
}

/// Local branch names, current branch first, then the rest sorted.
pub fn branches(dir: &Path) -> Vec<String> {
    let Some(out) = git_out(dir, &["for-each-ref", "--format=%(HEAD)%(refname)", "refs/heads"]) else {
        return Vec::new();
    };
    let mut current = None;
    let mut rest = Vec::new();
    for line in out.lines() {
        let (is_head, refname) = match line.strip_prefix('*') {
            Some(r) => (true, r),
            None => (false, line.strip_prefix(' ').unwrap_or(line)),
        };
        let name = refname.trim().strip_prefix("refs/heads/").unwrap_or(refname.trim());
        if name.is_empty() {
            continue;
        }
        if is_head && current.is_none() {
            current = Some(name.to_string());
        } else {
            rest.push(name.to_string());
        }
    }
    rest.sort();
    current.into_iter().chain(rest).collect()
}

// ---------------------------------------------------------------------------------------
// Folder scan
// ---------------------------------------------------------------------------------------

/// Scan a folder for agent-relevant files (no git calls except via `git_info`).
pub fn scan_folder(dir: &Path) -> FolderScan {
    let instructions = ["AGENTS.md", "CLAUDE.md"].iter().find_map(|name| {
        let text = std::fs::read(dir.join(name)).ok()?;
        Some((name.to_string(), String::from_utf8_lossy(&text).lines().count()))
    });

    let mut skills = BTreeSet::new();
    for root in [".harness/skills", ".claude/skills"] {
        let Ok(entries) = std::fs::read_dir(dir.join(root)) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("SKILL.md").is_file() {
                skills.insert(e.file_name().to_string_lossy().into_owned());
            }
        }
    }

    let mut commands = BTreeSet::new();
    for root in [".harness/commands", ".harness/command", ".claude/commands"] {
        let Ok(entries) = std::fs::read_dir(dir.join(root)) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file()
                && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))
                && let Some(stem) = p.file_stem()
            {
                commands.insert(stem.to_string_lossy().into_owned());
            }
        }
    }

    let mcp_servers = std::fs::read_to_string(dir.join(".mcp.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("mcpServers").and_then(|m| m.as_object()).map(|m| {
                let mut keys: Vec<String> = m.keys().cloned().collect();
                keys.sort();
                keys
            })
        })
        .unwrap_or_default();

    FolderScan {
        instructions,
        skills: skills.into_iter().collect(),
        commands: commands.into_iter().collect(),
        mcp_servers,
        git_status: git_info(dir).map(|g| g.summary()),
    }
}

// ---------------------------------------------------------------------------------------
// Branch names, worktrees, cloning
// ---------------------------------------------------------------------------------------

const SLUG_MAX: usize = 40;

/// Latin transliteration of a lowercase char; `None` if it has none (→ separator).
fn transliterate(c: char) -> Option<&'static str> {
    Some(match c {
        // Russian / Ukrainian / Belarusian Cyrillic.
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'ґ' => "g",
        'д' => "d",
        'е' => "e",
        'ё' => "e",
        'є' => "ye",
        'ж' => "zh",
        'з' => "z",
        'и' => "i",
        'і' => "i",
        'ї' => "yi",
        'й' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ў' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' | 'ь' => "",
        'ы' => "y",
        'э' => "e",
        'ю' => "yu",
        'я' => "ya",
        // Common Latin diacritics.
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'ł' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' => "r",
        'ś' | 'š' => "s",
        'ß' => "ss",
        'ť' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        // Apostrophes join words ("don't" → "dont").
        '\'' | '’' => "",
        _ => return None,
    })
}

/// `chat/<slug>` from a free-text title (lowercase ascii, `-` separated, ≤ 40 chars;
/// non-ascii transliterated or dropped; `chat/session` if nothing is left).
pub fn suggest_branch(title: &str) -> String {
    let mut slug = String::new();
    let mut pending_sep = false;
    for c in title.chars().flat_map(char::to_lowercase) {
        let mut buf = [0u8; 4];
        let piece: &str = if c.is_ascii_alphanumeric() {
            c.encode_utf8(&mut buf)
        } else if let Some(t) = transliterate(c) {
            t
        } else {
            pending_sep = true;
            continue;
        };
        if piece.is_empty() {
            continue;
        }
        if pending_sep && !slug.is_empty() {
            slug.push('-');
        }
        pending_sep = false;
        slug.push_str(piece);
    }
    if slug.len() > SLUG_MAX {
        // All ascii here, so byte slicing is safe; prefer cutting at a word boundary.
        let cut_at_word = slug.as_bytes()[SLUG_MAX] == b'-';
        slug.truncate(SLUG_MAX);
        if !cut_at_word && let Some(i) = slug.rfind('-').filter(|&i| i >= SLUG_MAX / 2) {
            slug.truncate(i);
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() { "chat/session".to_string() } else { format!("chat/{slug}") }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute())
}

fn worktree_path_in(base: &Path, repo_main_root: &Path, branch: &str) -> PathBuf {
    let repo = repo_main_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    base.join(repo).join(branch.replace('/', "-"))
}

/// Folder holding harness-created worktrees: `$HOME/.harness/worktrees`.
pub fn worktrees_base() -> PathBuf {
    home_dir().unwrap_or_else(std::env::temp_dir).join(".harness").join("worktrees")
}

/// Where new worktrees go: `$HOME/.harness/worktrees/<repo name>/<branch with / → ->`.
pub fn worktree_path_for(repo_main_root: &Path, branch: &str) -> PathBuf {
    worktree_path_in(&worktrees_base(), repo_main_root, branch)
}

/// `git worktree add -b <new_branch> <path> <from>`; returns the new worktree path.
/// Errors carry git's stderr.
pub fn create_worktree(repo: &Path, from: &str, new_branch: &str) -> anyhow::Result<PathBuf> {
    create_worktree_in(&worktrees_base(), repo, from, new_branch)
}

/// [`create_worktree`] with an explicit base folder instead of `~/.harness/worktrees`.
pub(crate) fn create_worktree_in(
    base: &Path,
    repo: &Path,
    from: &str,
    new_branch: &str,
) -> anyhow::Result<PathBuf> {
    let new_branch = new_branch.trim();
    let from = from.trim();
    if new_branch.is_empty() {
        bail!("branch name is empty");
    }
    if new_branch.starts_with('-') || from.starts_with('-') {
        bail!("invalid branch name `{new_branch}`");
    }
    let info = git_info(repo).ok_or_else(|| anyhow!("{} is not a git repository", repo.display()))?;
    let check = git_out(repo, &["check-ref-format", "--branch", new_branch]);
    if check.is_none() {
        bail!("invalid branch name `{new_branch}`");
    }
    let refname = format!("refs/heads/{new_branch}");
    if git_out(repo, &["rev-parse", "--verify", "--quiet", &refname]).is_some() {
        bail!("branch `{new_branch}` already exists");
    }
    let path = worktree_path_in(base, &info.main_root, new_branch);
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut cmd = git_cmd(repo);
    cmd.args(["worktree", "add", "-b", new_branch]).arg(&path);
    if !from.is_empty() {
        cmd.arg(from);
    }
    let out = run(cmd, WORKTREE_TIMEOUT).map_err(|e| anyhow!("git worktree add: {e}"))?;
    if !out.success {
        return Err(command_error("git worktree add", &out));
    }
    Ok(path)
}

/// Is `spec` a GitHub `owner/name` shorthand?
fn owner_name(spec: &str) -> Option<(&str, &str)> {
    let (owner, name) = spec.split_once('/')?;
    let valid = |s: &str| {
        !s.is_empty()
            && !s.starts_with(['.', '-'])
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(name)).then_some((owner, name))
}

/// Folder name for a clone of `url`: last path segment minus `.git`.
fn clone_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed).trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':', '\\']).next()?;
    let last = last.strip_suffix(".git").unwrap_or(last);
    (!last.is_empty() && last != "." && last != ".." && !last.starts_with('-')).then(|| last.to_string())
}

fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|d| {
        let p = d.join(program);
        std::fs::metadata(&p).is_ok_and(|m| {
            use std::os::unix::fs::PermissionsExt;
            m.is_file() && m.permissions().mode() & 0o111 != 0
        })
    })
}

/// Clone `spec` (`owner/name` → GitHub HTTPS, or any git URL) into `parent/<name>`.
/// Uses `gh repo clone` when `gh` is available and spec is `owner/name`, else `git clone`.
/// Returns the clone path; errors if the target exists.
pub fn clone_repo(spec: &str, parent: &Path) -> anyhow::Result<PathBuf> {
    let spec = spec.trim();
    if spec.is_empty() {
        bail!("nothing to clone");
    }
    if spec.starts_with('-') {
        bail!("invalid repository `{spec}`");
    }
    let shorthand = if spec.contains("://") { None } else { owner_name(spec) };
    let name = match shorthand {
        Some((_, name)) => name.strip_suffix(".git").unwrap_or(name).to_string(),
        None => clone_name(spec).ok_or_else(|| anyhow!("can't tell the repository name from `{}`", redact_credentials(spec)))?,
    };
    let target = parent.join(&name);
    if target.exists() {
        bail!("{} already exists", target.display());
    }
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let git_clone = |url: &str| -> anyhow::Result<()> {
        let mut cmd = Command::new("git");
        non_interactive(&mut cmd);
        cmd.current_dir(parent).args(["clone", "--"]).arg(url).arg(&target);
        let out = run(cmd, CLONE_TIMEOUT).map_err(|e| anyhow!("git clone: {}", redact_credentials(&e)))?;
        if out.success { Ok(()) } else { Err(command_error("git clone", &out)) }
    };

    match shorthand {
        Some((owner, repo)) => {
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            let full = format!("{owner}/{repo}");
            let mut gh_err = None;
            if on_path("gh") {
                let mut cmd = Command::new("gh");
                non_interactive(&mut cmd);
                cmd.current_dir(parent).args(["repo", "clone", &full]).arg(&target);
                match run(cmd, CLONE_TIMEOUT) {
                    Ok(out) if out.success => return Ok(target),
                    Ok(out) => gh_err = Some(command_error("gh repo clone", &out)),
                    Err(e) => gh_err = Some(anyhow!("gh repo clone: {e}")),
                }
                if target.exists() {
                    return Err(gh_err.expect("set above"));
                }
            }
            match git_clone(&format!("https://github.com/{full}.git")) {
                Ok(()) => Ok(target),
                Err(e) => Err(match gh_err {
                    Some(g) => anyhow!("{e} (after {g})"),
                    None => e,
                }),
            }
        }
        None => git_clone(spec).map(|()| target),
    }
}

// ---------------------------------------------------------------------------------------
// Paths and recent folders
// ---------------------------------------------------------------------------------------

/// Expand `~`, make absolute against `$HOME`, canonicalize; `None` unless it's a directory.
pub fn resolve_dir(input: &str) -> Option<PathBuf> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let path = if input == "~" {
        home_dir()?
    } else if let Some(rest) = input.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else {
        let p = PathBuf::from(input);
        if p.is_absolute() { p } else { home_dir()?.join(p) }
    };
    let path = path.canonicalize().ok()?;
    path.is_dir().then_some(path)
}

/// `~/code/x` style display path (home replaced by `~`).
pub fn display_path(path: &Path) -> String {
    if let Some(home) = home_dir().filter(|h| h.parent().is_some())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() { "~".to_string() } else { format!("~/{}", rest.display()) };
    }
    path.display().to_string()
}

const RECENT_MAX: usize = 30;

/// Recently used working folders, persisted in `$XDG_DATA_HOME/harness/workdirs.json`
/// (default `~/.local/share/...`), most recent first, max 30, missing folders dropped on load.
pub struct RecentWorkdirs {
    pub dirs: Vec<PathBuf>,
}

impl RecentWorkdirs {
    pub fn load() -> Self {
        match Self::default_path() {
            Some(p) => Self::load_from(&p),
            None => Self { dirs: Vec::new() },
        }
    }

    /// Move `dir` to the front and save (best effort).
    pub fn touch(&mut self, dir: &Path) {
        match Self::default_path() {
            Some(p) => {
                let _ = self.touch_at(dir, &p);
            }
            None => self.push_front(dir),
        }
    }

    fn default_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home_dir().map(|h| h.join(".local").join("share")))?;
        Some(data.join("harness").join("workdirs.json"))
    }

    /// Load from an explicit file (malformed/missing → empty).
    pub(crate) fn load_from(path: &Path) -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let parsed = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default();
        for d in parsed {
            let p = PathBuf::from(d);
            if p.is_dir() && !dirs.contains(&p) {
                dirs.push(p);
            }
            if dirs.len() >= RECENT_MAX {
                break;
            }
        }
        Self { dirs }
    }

    fn push_front(&mut self, dir: &Path) {
        self.dirs.retain(|d| d != dir);
        self.dirs.insert(0, dir.to_path_buf());
        self.dirs.truncate(RECENT_MAX);
    }

    /// [`Self::touch`] against an explicit file.
    pub(crate) fn touch_at(&mut self, dir: &Path, path: &Path) -> std::io::Result<()> {
        self.push_front(dir);
        self.save_to(path)
    }

    /// Atomic write: temp file in the same folder, then rename.
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let list: Vec<String> = self.dirs.iter().map(|d| d.to_string_lossy().into_owned()).collect();
        let json = serde_json::to_string_pretty(&list).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

#[cfg(test)]
#[path = "workdir_tests.rs"]
mod tests;
