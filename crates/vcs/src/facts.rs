//! What git knows about a folder: root, branch, remote, how dirty it is.

use std::path::{Path, PathBuf};

use base::cancel::CancelToken;
use model::RepoHost;

use crate::exec::Tool;
use crate::worktree;

/// The state of a repository checkout, as shown in the chat header and the new-chat page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoFacts {
    /// Top folder of this checkout (the worktree root for a linked worktree).
    pub root: PathBuf,
    /// Repository name: the folder name of the main checkout.
    pub name: String,
    /// Checked-out branch; `None` on a detached head.
    pub branch: Option<String>,
    /// Full hash of the head commit; `None` before the first commit.
    pub head: Option<String>,
    pub host: RepoHost,
    /// The `origin` URL (or the first remote's), credentials removed.
    pub remote_url: Option<String>,
    /// Changed plus untracked files.
    pub dirty: u32,
    /// Commits on the branch that `compared_to` does not have.
    pub ahead: u32,
    /// Commits on `compared_to` that the branch does not have.
    pub behind: u32,
    /// The folder is a linked worktree of another checkout.
    pub is_worktree: bool,
    /// The checkout a worktree was cut from; equals `root` for a plain checkout.
    pub main_checkout: PathBuf,
    /// `origin/HEAD`'s branch, else `main` or `master` when one exists.
    pub default_branch: Option<String>,
    /// What `ahead` and `behind` are counted against: the default branch (`main`, or
    /// `origin/main` when there is no local one) for a feature branch, the upstream for the
    /// default branch itself. `None` when there is nothing to compare with.
    pub compared_to: Option<String>,
}

/// Facts about the repository that contains `path`; `None` when it is not inside one.
pub fn facts(path: &Path, cancel: &CancelToken) -> Option<RepoFacts> {
    let dir = if path.is_file() { path.parent()? } else { path };
    if !dir.is_dir() {
        return None;
    }
    let out = Tool::git(dir)
        .args(["rev-parse", "--show-toplevel", "--absolute-git-dir", "--git-common-dir"])
        .query(cancel)?;
    let mut lines = out.lines().map(str::trim);
    let root = absolute(dir, lines.next().filter(|l| !l.is_empty())?);
    let git_dir = absolute(dir, lines.next().filter(|l| !l.is_empty())?);
    let common_dir = absolute(dir, lines.next().filter(|l| !l.is_empty())?);
    let is_worktree = git_dir != common_dir;
    let main_checkout = if is_worktree { main_checkout_of(dir, &common_dir, cancel) } else { root.clone() };

    let status = Tool::git(dir)
        .args(["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])
        .query(cancel)
        .map(|text| parse_status(&text));
    let status = match status {
        Some(status) => status,
        None => fallback_status(dir, cancel),
    };

    let (default_branch, local_default) = default_branch(dir, cancel);
    let remote_url = remote_url(dir, cancel);
    let host = remote_url.as_deref().map_or(RepoHost::Local, host_of);

    let mut facts = RepoFacts {
        name: repo_name(&main_checkout, &root),
        root,
        branch: status.branch.clone(),
        head: status.head.clone(),
        host,
        remote_url,
        dirty: status.dirty,
        ahead: 0,
        behind: 0,
        is_worktree,
        main_checkout,
        default_branch: default_branch.clone(),
        compared_to: None,
    };
    if let (Some(branch), Some(_)) = (&status.branch, &status.head) {
        match &default_branch {
            Some(default) if default != branch => {
                let reference = if local_default { default.clone() } else { format!("origin/{default}") };
                if let Some((behind, ahead)) = left_right(dir, &reference, cancel) {
                    facts.ahead = ahead;
                    facts.behind = behind;
                    facts.compared_to = Some(reference);
                }
            }
            _ => {
                if let Some(upstream) = &status.upstream {
                    facts.ahead = status.ahead;
                    facts.behind = status.behind;
                    facts.compared_to = Some(upstream.clone());
                }
            }
        }
    }
    Some(facts)
}

/// `github.com` is GitHub, `gitlab.*` is GitLab, anything else (or nothing) is `Local`.
/// Understands https, ssh://, git:// and `user@host:path` URLs.
pub fn host_of(remote_url: &str) -> RepoHost {
    match remote_host(remote_url).as_deref() {
        Some("github.com" | "www.github.com") => RepoHost::GitHub,
        Some(host) if host.starts_with("gitlab.") => RepoHost::GitLab,
        _ => RepoHost::Local,
    }
}

/// Local branch names, the current one first, the others sorted.
pub fn branches(repo: &Path, cancel: &CancelToken) -> Vec<String> {
    let Some(out) = Tool::git(repo)
        .args(["for-each-ref", "--format=%(HEAD)%(refname)", "refs/heads"])
        .query(cancel)
    else {
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

/// One line for the header: `2 uncommitted · 1 ahead of main`, `clean on main`.
pub fn status_line(facts: &RepoFacts) -> String {
    let mut parts = Vec::new();
    if facts.dirty > 0 {
        parts.push(format!("{} uncommitted", facts.dirty));
    }
    if let Some(target) = &facts.compared_to {
        if facts.ahead > 0 {
            parts.push(format!("{} ahead of {target}", facts.ahead));
        }
        if facts.behind > 0 {
            parts.push(format!("{} behind {target}", facts.behind));
        }
    }
    match (&facts.branch, parts.is_empty()) {
        (Some(branch), true) => format!("clean on {branch}"),
        (None, true) => format!("clean, {}", detached(facts)),
        (_, false) => {
            if facts.branch.is_none() {
                parts.push(detached(facts));
            }
            parts.join(" · ")
        }
    }
}

fn detached(facts: &RepoFacts) -> String {
    match &facts.head {
        Some(head) => format!("detached at {}", &head[..head.len().min(7)]),
        None => "detached".to_string(),
    }
}

/// Parsed `git status --porcelain=v2 --branch -z`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    pub head: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
}

pub(crate) fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    let mut records = text.split('\0');
    while let Some(record) = records.next() {
        if let Some(oid) = record.strip_prefix("# branch.oid ") {
            status.head = (oid != "(initial)").then(|| oid.to_string());
        } else if let Some(head) = record.strip_prefix("# branch.head ") {
            status.branch = (head != "(detached)").then(|| head.to_string());
        } else if let Some(upstream) = record.strip_prefix("# branch.upstream ") {
            status.upstream = Some(upstream.to_string());
        } else if let Some(ab) = record.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        } else if record.starts_with("2 ") {
            // A rename or copy is followed by a record with the original path.
            status.dirty += 1;
            records.next();
        } else if record.starts_with("1 ") || record.starts_with("u ") || record.starts_with("? ") {
            status.dirty += 1;
        }
    }
    status
}

/// Branch and head when `git status` itself failed (a huge tree, a timeout).
fn fallback_status(dir: &Path, cancel: &CancelToken) -> Status {
    let head = Tool::git(dir)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .query(cancel)
        .and_then(|s| first_line(&s));
    let branch = Tool::git(dir)
        .args(["symbolic-ref", "--short", "--quiet", "HEAD"])
        .query(cancel)
        .and_then(|s| first_line(&s));
    Status { head, branch, ..Status::default() }
}

/// The default branch and whether a local branch of that name exists.
fn default_branch(dir: &Path, cancel: &CancelToken) -> (Option<String>, bool) {
    let Some(out) = Tool::git(dir)
        .args([
            "for-each-ref",
            "--format=%(refname)%09%(symref)",
            "refs/remotes/origin/HEAD",
            "refs/heads/main",
            "refs/heads/master",
            "refs/remotes/origin/main",
            "refs/remotes/origin/master",
        ])
        .query(cancel)
    else {
        return (None, false);
    };
    parse_default_branch(&out)
}

pub(crate) fn parse_default_branch(out: &str) -> (Option<String>, bool) {
    let (mut origin_head, mut local_main, mut local_master, mut remote_main, mut remote_master) =
        (None, false, false, false, false);
    for line in out.lines() {
        let (name, symref) = line.split_once('\t').unwrap_or((line, ""));
        match name.trim() {
            "refs/remotes/origin/HEAD" => {
                origin_head = symref
                    .trim()
                    .strip_prefix("refs/remotes/origin/")
                    .filter(|b| !b.is_empty())
                    .map(str::to_string);
            }
            "refs/heads/main" => local_main = true,
            "refs/heads/master" => local_master = true,
            "refs/remotes/origin/main" => remote_main = true,
            "refs/remotes/origin/master" => remote_master = true,
            _ => {}
        }
    }
    let default = origin_head.or_else(|| {
        if local_main || remote_main {
            Some("main".to_string())
        } else if local_master || remote_master {
            Some("master".to_string())
        } else {
            None
        }
    });
    let local = match default.as_deref() {
        Some("main") => local_main,
        Some("master") => local_master,
        // A custom default branch from origin/HEAD: assume it has a local branch.
        Some(_) => true,
        None => false,
    };
    (default, local)
}

/// `(behind, ahead)` of HEAD against `reference`.
fn left_right(dir: &Path, reference: &str, cancel: &CancelToken) -> Option<(u32, u32)> {
    let out = Tool::git(dir)
        .args(["rev-list", "--left-right", "--count"])
        .arg(format!("{reference}...HEAD"))
        .query(cancel)?;
    let mut numbers = out.split_whitespace().map(|n| n.parse::<u32>().ok());
    Some((numbers.next()??, numbers.next()??))
}

fn remote_url(dir: &Path, cancel: &CancelToken) -> Option<String> {
    let url = Tool::git(dir)
        .args(["remote", "get-url", "origin"])
        .query(cancel)
        .and_then(|s| first_line(&s))
        .or_else(|| {
            let remotes = Tool::git(dir).arg("remote").query(cancel)?;
            let name = first_line(&remotes)?;
            first_line(&Tool::git(dir).args(["remote", "get-url", &name]).query(cancel)?)
        })?;
    Some(crate::exec::redact_credentials(&url))
}

/// The main checkout of the repository a linked worktree belongs to.
fn main_checkout_of(dir: &Path, common_dir: &Path, cancel: &CancelToken) -> PathBuf {
    let from_list = Tool::git(dir)
        .args(["worktree", "list", "--porcelain"])
        .query(cancel)
        .and_then(|text| worktree::parse_porcelain(&text).into_iter().next())
        .map(|entry| entry.path);
    from_list.unwrap_or_else(|| match common_dir.file_name() {
        Some(name) if name == ".git" => common_dir.parent().map_or_else(|| common_dir.to_path_buf(), Path::to_path_buf),
        _ => common_dir.to_path_buf(),
    })
}

fn repo_name(main_checkout: &Path, root: &Path) -> String {
    let folder = main_checkout.file_name().or_else(|| root.file_name());
    match folder {
        Some(name) => {
            let name = name.to_string_lossy();
            // The common dir of a bare repository is `repo.git`.
            name.strip_suffix(".git").filter(|n| !n.is_empty()).unwrap_or(&name).to_string()
        }
        None => root.display().to_string(),
    }
}

fn absolute(base: &Path, text: &str) -> PathBuf {
    let path = Path::new(text);
    let joined = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    joined.canonicalize().unwrap_or(joined)
}

fn first_line(text: &str) -> Option<String> {
    text.lines().next().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string)
}

/// The host of a remote URL, lowercase, without port or user.
pub(crate) fn remote_host(url: &str) -> Option<String> {
    let url = url.trim();
    let host = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let authority = rest.split('/').next().unwrap_or(rest);
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        if host.starts_with('[') {
            host.split_once(']').map_or(host, |(h, _)| h.trim_start_matches('['))
        } else {
            host.split_once(':').map_or(host, |(h, _)| h)
        }
    } else {
        // scp-style `[user@]host:path`: nothing before the colon may be a path.
        let (before, _) = url.split_once(':')?;
        if before.contains('/') || before.is_empty() {
            return None;
        }
        before.rsplit_once('@').map_or(before, |(_, h)| h)
    };
    let host = host.trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, commit_file, git, write};

    fn cancel() -> CancelToken {
        CancelToken::new()
    }

    #[test]
    fn host_of_recognises_the_big_hosts() {
        assert_eq!(host_of("https://github.com/acme/api.git"), RepoHost::GitHub);
        assert_eq!(host_of("https://user:tok@github.com/acme/api"), RepoHost::GitHub);
        assert_eq!(host_of("git@github.com:acme/api.git"), RepoHost::GitHub);
        assert_eq!(host_of("ssh://git@GitHub.com:22/acme/api.git"), RepoHost::GitHub);
        assert_eq!(host_of("git://github.com/acme/api.git"), RepoHost::GitHub);
        assert_eq!(host_of("https://gitlab.com/group/sub/repo.git"), RepoHost::GitLab);
        assert_eq!(host_of("git@gitlab.example.org:team/repo.git"), RepoHost::GitLab);
        assert_eq!(host_of("https://git.example.com/team/repo.git"), RepoHost::Local);
        assert_eq!(host_of("https://notgithub.com/a/b"), RepoHost::Local);
        assert_eq!(host_of("/srv/git/repo.git"), RepoHost::Local);
        assert_eq!(host_of("file:///srv/git/repo.git"), RepoHost::Local);
        assert_eq!(host_of(""), RepoHost::Local);
    }

    #[test]
    fn a_folder_that_is_not_a_repo_has_no_facts() {
        let sandbox = Sandbox::new();
        assert_eq!(facts(sandbox.path(), &cancel()), None);
        assert_eq!(facts(&sandbox.path().join("missing"), &cancel()), None);
    }

    #[test]
    fn fresh_repo_without_commits() {
        let sandbox = Sandbox::new();
        let repo = sandbox.path().join("fresh");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(f.root, repo);
        assert_eq!(f.name, "fresh");
        assert_eq!(f.branch.as_deref(), Some("main"));
        assert_eq!(f.head, None);
        assert_eq!((f.dirty, f.ahead, f.behind), (0, 0, 0));
        assert_eq!(f.default_branch, None);
        assert_eq!(f.host, RepoHost::Local);
        assert!(!f.is_worktree);
        assert_eq!(f.main_checkout, repo);
        assert_eq!(status_line(&f), "clean on main");
    }

    #[test]
    fn clean_repo_with_one_commit() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("payments-api");
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(f.name, "payments-api");
        assert_eq!(f.branch.as_deref(), Some("main"));
        assert_eq!(f.head.as_deref().map(str::len), Some(40));
        assert_eq!(f.dirty, 0);
        assert_eq!(f.default_branch.as_deref(), Some("main"));
        assert_eq!(f.compared_to, None);
        assert_eq!(status_line(&f), "clean on main");
        // A subfolder resolves to the same repo.
        write(&repo, "src/lib.rs", "");
        let sub = repo.join("src");
        assert_eq!(facts(&sub, &cancel()).expect("facts").root, repo);
        // A file path works too.
        assert_eq!(facts(&repo.join("README.md"), &cancel()).expect("facts").root, repo);
    }

    #[test]
    fn dirty_repo_counts_changed_untracked_and_renamed_files_once() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("dirty");
        commit_file(&repo, "a.txt", "one\ntwo\nthree\nfour\n");
        write(&repo, "README.md", "changed\n");
        write(&repo, "new/one.txt", "x");
        write(&repo, "new/two.txt", "y");
        git(&repo, &["mv", "a.txt", "b.txt"]);
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(f.dirty, 4, "README, two untracked, one rename");
        assert_eq!(status_line(&f), "4 uncommitted");
    }

    #[test]
    fn feature_branch_is_compared_with_the_default_branch() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("feature");
        git(&repo, &["checkout", "-q", "-b", "chat/x"]);
        commit_file(&repo, "x.txt", "x");
        write(&repo, "y.txt", "y");
        write(&repo, "z.txt", "z");
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(f.branch.as_deref(), Some("chat/x"));
        assert_eq!((f.ahead, f.behind), (1, 0));
        assert_eq!(f.compared_to.as_deref(), Some("main"));
        assert_eq!(status_line(&f), "2 uncommitted · 1 ahead of main");

        git(&repo, &["checkout", "-q", "main"]);
        write(&repo, "m.txt", "m");
        git(&repo, &["add", "m.txt"]);
        git(&repo, &["commit", "-q", "-m", "m"]);
        git(&repo, &["checkout", "-q", "chat/x"]);
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!((f.ahead, f.behind), (1, 1));
        assert_eq!(status_line(&f), "2 uncommitted · 1 ahead of main · 1 behind main");
    }

    #[test]
    fn default_branch_is_compared_with_its_upstream_and_remote_is_read() {
        let sandbox = Sandbox::new();
        let remote = sandbox.bare("remote.git");
        let repo = sandbox.repo("local");
        git(&repo, &["remote", "add", "origin", "https://user:secret@github.com/acme/local.git"]);
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(f.host, RepoHost::GitHub);
        assert_eq!(f.remote_url.as_deref(), Some("https://github.com/acme/local.git"));

        git(&repo, &["remote", "set-url", "origin", remote.to_str().unwrap()]);
        git(&repo, &["push", "-q", "-u", "origin", "main"]);
        commit_file(&repo, "a.txt", "a");
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(f.host, RepoHost::Local);
        assert_eq!((f.ahead, f.behind), (1, 0));
        assert_eq!(f.compared_to.as_deref(), Some("origin/main"));
        assert_eq!(status_line(&f), "1 ahead of origin/main");
    }

    #[test]
    fn linked_worktree_knows_its_main_checkout() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("main-repo");
        let wt = sandbox.path().join("wt");
        git(&repo, &["worktree", "add", "-q", "-b", "chat/y", wt.to_str().unwrap()]);
        let f = facts(&wt, &cancel()).expect("facts");
        assert!(f.is_worktree);
        assert_eq!(f.root, wt);
        assert_eq!(f.main_checkout, repo);
        assert_eq!(f.name, "main-repo");
        assert_eq!(f.branch.as_deref(), Some("chat/y"));
        let main = facts(&repo, &cancel()).expect("facts");
        assert!(!main.is_worktree);
        assert_eq!(main.main_checkout, repo);
    }

    #[test]
    fn detached_head() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("detached");
        git(&repo, &["checkout", "-q", "--detach"]);
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(f.branch, None);
        let head = f.head.clone().expect("head");
        assert_eq!(status_line(&f), format!("clean, detached at {}", &head[..7]));
        write(&repo, "x", "x");
        let f = facts(&repo, &cancel()).expect("facts");
        assert_eq!(status_line(&f), format!("1 uncommitted · detached at {}", &head[..7]));
    }

    #[test]
    fn branches_list_current_first() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("branches");
        git(&repo, &["branch", "zeta"]);
        git(&repo, &["branch", "alpha"]);
        git(&repo, &["checkout", "-q", "-b", "mid"]);
        assert_eq!(branches(&repo, &cancel()), ["mid", "alpha", "main", "zeta"]);
        assert!(branches(sandbox.path(), &cancel()).is_empty());
    }

    #[test]
    fn status_parser_handles_renames_and_headers() {
        let text = "# branch.oid abc\0# branch.head feat\0# branch.upstream origin/feat\0# branch.ab +2 -1\0\
            1 .M N... 100644 100644 100644 a b src/a.rs\0\
            2 R. N... 100644 100644 100644 a b R100 new.rs\0old.rs\0\
            ? untracked.txt\0u UU N... 1 2 3 4 a b c conflict.rs\0";
        let s = parse_status(text);
        assert_eq!(s.head.as_deref(), Some("abc"));
        assert_eq!(s.branch.as_deref(), Some("feat"));
        assert_eq!(s.upstream.as_deref(), Some("origin/feat"));
        assert_eq!((s.ahead, s.behind, s.dirty), (2, 1, 4));
    }

    #[test]
    fn default_branch_parser_prefers_origin_head() {
        let out = "refs/remotes/origin/HEAD\trefs/remotes/origin/develop\nrefs/heads/main\t\n";
        assert_eq!(parse_default_branch(out), (Some("develop".into()), true));
        assert_eq!(parse_default_branch("refs/heads/master\t\n"), (Some("master".into()), true));
        assert_eq!(parse_default_branch("refs/remotes/origin/main\t\n"), (Some("main".into()), false));
        assert_eq!(parse_default_branch(""), (None, false));
    }
}
