use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agent_tools_workdir_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run git in `dir` with an isolated config; panics on failure.
fn sh_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repo on `main` with one commit.
fn init_repo(t: &TempDir, name: &str) -> PathBuf {
    let repo = t.path().join(name);
    std::fs::create_dir_all(&repo).unwrap();
    sh_git(&repo, &["init", "-q"]);
    sh_git(&repo, &["checkout", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    sh_git(&repo, &["add", "."]);
    sh_git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

fn commit_file(repo: &Path, file: &str) {
    std::fs::write(repo.join(file), file).unwrap();
    sh_git(repo, &["add", "."]);
    sh_git(repo, &["commit", "-q", "-m", file]);
}

#[test]
fn remote_parsing() {
    let gh = |p: &str| Some((Provider::GitHub, p.to_string()));
    assert_eq!(parse_remote("https://github.com/acme/payments-api.git"), gh("github.com/acme/payments-api"));
    assert_eq!(parse_remote("https://github.com/acme/payments-api"), gh("github.com/acme/payments-api"));
    assert_eq!(parse_remote("https://github.com/acme/payments-api/"), gh("github.com/acme/payments-api"));
    assert_eq!(parse_remote("https://user:tok@github.com/acme/x.git"), gh("github.com/acme/x"));
    assert_eq!(parse_remote("git@github.com:acme/x.git"), gh("github.com/acme/x"));
    assert_eq!(parse_remote("github.com:acme/x"), gh("github.com/acme/x"));
    assert_eq!(parse_remote("ssh://git@github.com/acme/x.git"), gh("github.com/acme/x"));
    assert_eq!(parse_remote("ssh://git@GitHub.com:22/acme/x"), gh("github.com/acme/x"));
    assert_eq!(parse_remote("git://github.com/acme/x.git"), gh("github.com/acme/x"));
    assert_eq!(
        parse_remote("https://gitlab.com/group/sub/repo.git"),
        Some((Provider::GitLab, "gitlab.com/group/sub/repo".into()))
    );
    assert_eq!(
        parse_remote("git@gitlab.example.org:team/repo.git"),
        Some((Provider::GitLab, "gitlab.example.org/team/repo".into()))
    );
    assert_eq!(
        parse_remote("git@bitbucket.org:team/repo.git"),
        Some((Provider::Bitbucket, "bitbucket.org/team/repo".into()))
    );
    assert_eq!(
        parse_remote("https://git.example.com:8443/team/repo.git"),
        Some((Provider::Other("git.example.com".into()), "git.example.com/team/repo".into()))
    );
    assert_eq!(parse_remote("/srv/git/repo.git"), None);
    assert_eq!(parse_remote("../other"), None);
    assert_eq!(parse_remote("file:///srv/git/repo.git"), None);
    assert_eq!(parse_remote("https://github.com"), None);
    assert_eq!(parse_remote(""), None);
    assert_eq!(Provider::Other("git.example.com".into()).label(), "git.example.com");
}

#[test]
fn credentials_redacted() {
    assert_eq!(
        redact_credentials("fatal: repository 'https://me:s3cret@github.com/a/b' not found"),
        "fatal: repository 'https://github.com/a/b' not found"
    );
    assert_eq!(redact_credentials("git@github.com:a/b"), "git@github.com:a/b");
}

fn info(branch: Option<&str>, changed: usize, ahead: usize, behind: usize) -> GitInfo {
    GitInfo {
        root: "/r".into(),
        main_root: "/r".into(),
        is_linked_worktree: false,
        branch: branch.map(str::to_string),
        default_branch: Some("main".into()),
        remote_url: None,
        provider: None,
        remote_path: None,
        changed,
        ahead,
        behind,
    }
}

#[test]
fn summaries() {
    assert_eq!(info(Some("main"), 0, 0, 0).summary(), "clean on main");
    assert_eq!(info(Some("feat/x"), 3, 0, 0).summary(), "3 changed on feat/x");
    assert_eq!(info(Some("main"), 0, 1, 0).summary(), "clean on main · 1 ahead");
    assert_eq!(info(Some("main"), 2, 1, 2).summary(), "2 changed on main · 1 ahead · 2 behind");
    assert_eq!(info(None, 0, 0, 0).summary(), "detached HEAD");
    assert_eq!(info(None, 4, 0, 0).summary(), "4 changed · detached HEAD");
}

#[test]
fn branch_suggestions() {
    assert_eq!(suggest_branch("Fix login bug"), "chat/fix-login-bug");
    assert_eq!(suggest_branch("  Fix: the *login* bug!! "), "chat/fix-the-login-bug");
    assert_eq!(suggest_branch("Починить логин"), "chat/pochinit-login");
    assert_eq!(suggest_branch("Щёлк и Юла: съезд"), "chat/shchelk-i-yula-sezd");
    assert_eq!(suggest_branch("Café déjà vu"), "chat/cafe-deja-vu");
    assert_eq!(suggest_branch("don't panic"), "chat/dont-panic");
    assert_eq!(suggest_branch("v2.0 release"), "chat/v2-0-release");
    assert_eq!(suggest_branch(""), "chat/session");
    assert_eq!(suggest_branch("🚀 ✨"), "chat/session");
    assert_eq!(suggest_branch("日本 api"), "chat/api");
    let long = suggest_branch("implement the new payment reconciliation pipeline for all regions");
    let slug = long.strip_prefix("chat/").unwrap();
    assert!(slug.len() <= 40, "{long}");
    assert_eq!(slug, "implement-the-new-payment-reconciliation");
    let one_word = suggest_branch(&"a".repeat(100));
    assert_eq!(one_word.len(), "chat/".len() + 40);
    assert!(!suggest_branch("abc-------------------------------------------------def").ends_with('-'));
}

#[test]
fn status_and_refs_parsing() {
    let st = parse_status_v2(
        "# branch.oid abc\n# branch.head feat/x\n# branch.upstream origin/feat/x\n# branch.ab +2 -5\n\
         1 .M N... 100644 100644 100644 a b src/a.rs\n? new.txt\n",
    );
    assert_eq!(st, StatusFacts { head: Some(Some("feat/x".into())), changed: 2, ahead: 2, behind: 5 });
    let st = parse_status_v2("# branch.oid abc\n# branch.head (detached)\n");
    assert_eq!(st.head, Some(None));
    assert_eq!(st.changed, 0);

    assert_eq!(
        parse_default_branch("refs/heads/main\t\nrefs/remotes/origin/HEAD\trefs/remotes/origin/develop\n"),
        Some("develop".into())
    );
    assert_eq!(parse_default_branch("refs/heads/master\t\n"), Some("master".into()));
    assert_eq!(parse_default_branch("refs/heads/master\t\nrefs/heads/main\t\n"), Some("main".into()));
    assert_eq!(parse_default_branch(""), None);

    let wts = parse_worktrees(
        "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /w/x\nHEAD def\ndetached\n\n\
         worktree /w/y\nHEAD 123\nbranch refs/heads/feat/y\nlocked\n\n",
    );
    assert_eq!(
        wts,
        vec![
            Worktree { path: "/r".into(), branch: Some("main".into()), is_main: true },
            Worktree { path: "/w/x".into(), branch: None, is_main: false },
            Worktree { path: "/w/y".into(), branch: Some("feat/y".into()), is_main: false },
        ]
    );
    let bare = parse_worktrees("worktree /r.git\nbare\n\nworktree /w/x\nHEAD def\nbranch refs/heads/x\n");
    assert_eq!(bare, vec![Worktree { path: "/w/x".into(), branch: Some("x".into()), is_main: false }]);
}

#[test]
fn clone_names() {
    assert_eq!(owner_name("acme/payments-api"), Some(("acme", "payments-api")));
    assert_eq!(owner_name("acme/x.git"), Some(("acme", "x.git")));
    assert_eq!(owner_name("git@github.com:acme/x"), None);
    assert_eq!(owner_name("./acme/x"), None);
    assert_eq!(owner_name("a/b/c"), None);
    assert_eq!(clone_name("https://github.com/acme/x.git"), Some("x".into()));
    assert_eq!(clone_name("git@github.com:acme/x.git"), Some("x".into()));
    assert_eq!(clone_name("https://host/acme/x/"), Some("x".into()));
    assert_eq!(clone_name("/srv/git/repo.git"), Some("repo".into()));
    assert_eq!(clone_name("https://host/"), Some("host".into()));
    assert_eq!(clone_name(".."), None);
}

#[test]
fn not_a_repo() {
    if !have_git() {
        return;
    }
    let t = TempDir::new();
    let missing = t.path().join("missing");
    assert_eq!(git_info(&missing), None);
    assert!(worktrees(&missing).is_empty());
    assert!(branches(&missing).is_empty());
}

#[test]
fn repo_facts() {
    if !have_git() {
        return;
    }
    let t = TempDir::new();
    let repo = init_repo(&t, "proj");
    let g = git_info(&repo).expect("repo");
    assert_eq!(g.root, repo);
    assert_eq!(g.main_root, repo);
    assert!(!g.is_linked_worktree);
    assert_eq!(g.branch.as_deref(), Some("main"));
    assert_eq!(g.default_branch.as_deref(), Some("main"));
    assert_eq!((g.changed, g.ahead, g.behind), (0, 0, 0));
    assert_eq!(g.remote_url, None);
    assert_eq!(g.summary(), "clean on main");

    // Subfolder resolves to the same root; changes are counted (untracked files too).
    std::fs::create_dir_all(repo.join("src/deep")).unwrap();
    std::fs::write(repo.join("src/deep/a.txt"), "a").unwrap();
    std::fs::write(repo.join("src/deep/b.txt"), "b").unwrap();
    std::fs::write(repo.join("README.md"), "changed\n").unwrap();
    let g = git_info(&repo.join("src")).unwrap();
    assert_eq!(g.root, repo);
    assert_eq!(g.changed, 3);
    assert_eq!(g.summary(), "3 changed on main");
    sh_git(&repo, &["add", "."]);
    sh_git(&repo, &["commit", "-q", "-m", "more"]);

    // Remote + upstream tracking a local branch.
    sh_git(&repo, &["remote", "add", "origin", "git@github.com:acme/proj.git"]);
    sh_git(&repo, &["branch", "base"]);
    commit_file(&repo, "x.txt");
    sh_git(&repo, &["branch", "-q", "--set-upstream-to=base"]);
    let g = git_info(&repo).unwrap();
    assert_eq!(g.provider, Some(Provider::GitHub));
    assert_eq!(g.remote_path.as_deref(), Some("github.com/acme/proj"));
    assert_eq!(g.remote_url.as_deref(), Some("git@github.com:acme/proj.git"));
    assert_eq!((g.ahead, g.behind), (1, 0));
    assert_eq!(g.summary(), "clean on main · 1 ahead");
    sh_git(&repo, &["checkout", "-q", "base"]);
    sh_git(&repo, &["branch", "-q", "--set-upstream-to=main"]);
    assert_eq!(git_info(&repo).unwrap().summary(), "clean on base · 1 behind");

    assert_eq!(branches(&repo), vec!["base".to_string(), "main".to_string()]);

    sh_git(&repo, &["checkout", "-q", "--detach"]);
    let g = git_info(&repo).unwrap();
    assert_eq!(g.branch, None);
    assert_eq!(g.summary(), "detached HEAD");
}

#[test]
fn default_branch_from_origin_head() {
    if !have_git() {
        return;
    }
    let t = TempDir::new();
    let repo = init_repo(&t, "proj");
    sh_git(&repo, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
    sh_git(&repo, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk"]);
    assert_eq!(git_info(&repo).unwrap().default_branch.as_deref(), Some("trunk"));
}

#[test]
fn linked_worktree() {
    if !have_git() {
        return;
    }
    let t = TempDir::new();
    let repo = init_repo(&t, "proj");
    let base = t.path().join("wt-base");
    let path = create_worktree_in(&base, &repo, "main", "chat/fix-login").unwrap();
    assert_eq!(path, base.join("proj").join("chat-fix-login"));
    assert!(path.join("README.md").is_file());

    let g = git_info(&path).unwrap();
    assert!(g.is_linked_worktree);
    assert_eq!(g.root, path);
    assert_eq!(g.main_root, repo);
    assert_eq!(g.branch.as_deref(), Some("chat/fix-login"));

    let wts = worktrees(&path);
    assert_eq!(wts.len(), 2);
    assert_eq!(wts[0].path.canonicalize().unwrap(), repo);
    assert!(wts[0].is_main);
    assert_eq!(wts[0].branch.as_deref(), Some("main"));
    assert_eq!(wts[1].path.canonicalize().unwrap(), path);
    assert!(!wts[1].is_main);

    assert_eq!(branches(&path), vec!["chat/fix-login".to_string(), "main".to_string()]);

    // Same branch again → error; occupied path → error; bad base ref → git's stderr.
    let err = create_worktree_in(&base, &repo, "main", "chat/fix-login").unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    std::fs::create_dir_all(base.join("proj").join("taken")).unwrap();
    let err = create_worktree_in(&base, &repo, "main", "taken").unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    let err = create_worktree_in(&base, &repo, "no-such-ref", "other").unwrap_err();
    assert!(err.to_string().starts_with("git worktree add failed:"), "{err}");
    assert!(create_worktree_in(&base, &repo, "main", "-x").is_err());
    assert!(create_worktree_in(&base, &repo, "main", "bad..name").is_err());
    assert!(create_worktree_in(&base, &t.path().join("nope"), "main", "y").is_err());

    assert_eq!(
        worktree_path_for(Path::new("/code/proj"), "chat/a/b").file_name().unwrap(),
        "chat-a-b"
    );
    assert!(worktree_path_for(Path::new("/code/proj"), "x").ends_with(".harness/worktrees/proj/x"));
}

#[test]
fn clone_local_repo() {
    if !have_git() {
        return;
    }
    let t = TempDir::new();
    let repo = init_repo(&t, "source.git");
    let parent = t.path().join("clones");
    let cloned = clone_repo(repo.to_str().unwrap(), &parent).unwrap();
    assert_eq!(cloned, parent.join("source"));
    assert!(cloned.join("README.md").is_file());
    let err = clone_repo(repo.to_str().unwrap(), &parent).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    assert!(clone_repo("", &parent).is_err());
    assert!(clone_repo("--upload-pack=x", &parent).is_err());
    let err = clone_repo(t.path().join("missing").to_str().unwrap(), &parent).unwrap_err();
    assert!(err.to_string().starts_with("git clone failed"), "{err}");
}

#[test]
fn folder_scan() {
    let t = TempDir::new();
    assert_eq!(scan_folder(t.path()).instructions, None);
    t.write("CLAUDE.md", "one\ntwo\n");
    assert_eq!(scan_folder(t.path()).instructions, Some(("CLAUDE.md".into(), 2)));
    t.write("AGENTS.md", "a\nb\nc");
    t.write(".harness/skills/deploy/SKILL.md", "---\nname: deploy\n---\n");
    t.write(".claude/skills/deploy/SKILL.md", "x");
    t.write(".claude/skills/review/SKILL.md", "x");
    t.write(".claude/skills/not-a-skill/README.md", "x");
    t.write(".harness/commands/ship.md", "x");
    t.write(".harness/command/lint.md", "x");
    t.write(".claude/commands/ship.md", "x");
    t.write(".claude/commands/notes.txt", "x");
    t.write(".mcp.json", r#"{"mcpServers": {"zeta": {"command": "z"}, "alpha": {"url": "u"}}}"#);
    let scan = scan_folder(t.path());
    assert_eq!(scan.instructions, Some(("AGENTS.md".into(), 3)));
    assert_eq!(scan.skills, vec!["deploy", "review"]);
    assert_eq!(scan.commands, vec!["lint", "ship"]);
    assert_eq!(scan.mcp_servers, vec!["alpha", "zeta"]);

    t.write(".mcp.json", "{not json");
    assert!(scan_folder(t.path()).mcp_servers.is_empty());

    if have_git() {
        let repo = init_repo(&t, "repo");
        assert_eq!(scan_folder(&repo).git_status.as_deref(), Some("clean on main"));
    }
}

#[test]
fn recent_round_trip() {
    let t = TempDir::new();
    let file = t.path().join("data/harness/workdirs.json");
    let a = t.path().join("a");
    let b = t.path().join("b");
    let gone = t.path().join("gone");
    for d in [&a, &b, &gone] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut r = RecentWorkdirs::load_from(&file);
    assert!(r.dirs.is_empty());
    r.touch_at(&a, &file).unwrap();
    r.touch_at(&gone, &file).unwrap();
    r.touch_at(&b, &file).unwrap();
    r.touch_at(&a, &file).unwrap();
    assert_eq!(r.dirs, vec![a.clone(), b.clone(), gone.clone()]);
    let text = std::fs::read_to_string(&file).unwrap();
    let json: Vec<String> = serde_json::from_str(&text).unwrap();
    assert_eq!(json.len(), 3);

    std::fs::remove_dir_all(&gone).unwrap();
    let r = RecentWorkdirs::load_from(&file);
    assert_eq!(r.dirs, vec![a.clone(), b.clone()]);

    let mut r = RecentWorkdirs::load_from(&file);
    for i in 0..40 {
        let d = t.path().join(format!("d{i}"));
        std::fs::create_dir_all(&d).unwrap();
        r.touch_at(&d, &file).unwrap();
    }
    assert_eq!(r.dirs.len(), 30);
    assert_eq!(RecentWorkdirs::load_from(&file).dirs.len(), 30);
    assert_eq!(r.dirs[0], t.path().join("d39"));

    std::fs::write(&file, "garbage").unwrap();
    assert!(RecentWorkdirs::load_from(&file).dirs.is_empty());
    // No temp files left behind.
    let leftovers = std::fs::read_dir(file.parent().unwrap()).unwrap().count();
    assert_eq!(leftovers, 1);
}

#[test]
fn paths() {
    let t = TempDir::new();
    let f = t.write("file.txt", "x");
    assert_eq!(resolve_dir(t.path().to_str().unwrap()), Some(t.path().to_path_buf()));
    assert_eq!(resolve_dir(&format!("  {}/./  ", t.path().display())), Some(t.path().to_path_buf()));
    assert_eq!(resolve_dir(f.to_str().unwrap()), None);
    assert_eq!(resolve_dir(t.path().join("missing").to_str().unwrap()), None);
    assert_eq!(resolve_dir(""), None);

    if let Some(home) = home_dir().filter(|h| h.parent().is_some() && h.is_dir()) {
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(&home.join("code/x")), "~/code/x");
        assert_eq!(resolve_dir("~"), home.canonicalize().ok());
    }
    assert_eq!(display_path(Path::new("/definitely/not/home")), "/definitely/not/home");
}
