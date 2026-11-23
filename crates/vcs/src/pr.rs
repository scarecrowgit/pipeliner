//! Pull requests on GitHub through the `gh` command line.
//!
//! Running `gh` sits behind the [`Gh`] trait, so tests and other crates can feed canned
//! output; [`create`] and [`view`] use the real [`Cli`].

use std::path::Path;

use anyhow::{Result, anyhow, bail};
use base::cancel::CancelToken;
use model::PullRequest;
use serde_json::Value;

use crate::exec::{Tool, redact_credentials};

/// The fields `gh pr view --json` is asked for.
const VIEW_FIELDS: &str =
    "url,number,title,state,isDraft,reviewDecision,statusCheckRollup,additions,deletions,headRefName,baseRefName";

/// What one `gh` call printed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GhOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Something that can run `gh` with arguments, in a folder, with optional input.
pub trait Gh {
    /// Runs `gh args…`. A command that ran and failed is an `Ok` with `success: false`;
    /// `Err` is for a `gh` that could not run at all.
    fn run(&self, dir: Option<&Path>, args: &[String], stdin: Option<&str>, cancel: &CancelToken) -> Result<GhOutput>;
}

/// The installed `gh`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cli;

impl Gh for Cli {
    fn run(&self, dir: Option<&Path>, args: &[String], stdin: Option<&str>, cancel: &CancelToken) -> Result<GhOutput> {
        let mut tool = Tool::gh(dir).args(args);
        if let Some(text) = stdin {
            tool = tool.stdin(text);
        }
        let out = tool.run(cancel)?;
        Ok(GhOutput { success: out.success(), stdout: out.stdout, stderr: out.stderr })
    }
}

/// Whether a `gh` executable is on the `PATH`. Says nothing about being logged in.
pub fn gh_available() -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| {
        std::fs::metadata(dir.join("gh")).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// A pull request to open.
pub struct NewPr<'a> {
    pub title: &'a str,
    pub body: &'a str,
    /// The branch the pull request merges into.
    pub base: &'a str,
    pub draft: bool,
}

/// Opens a pull request for the branch checked out in `worktree` and returns its URL. When an
/// open one exists for the branch already, its URL is returned and nothing is created. The
/// branch has to be pushed first (see [`crate::push`]).
pub fn create(worktree: &Path, pr: &NewPr<'_>, cancel: &CancelToken) -> Result<String> {
    create_with(&Cli, worktree, pr, cancel)
}

/// [`create`] with a custom `gh`.
pub fn create_with(gh: &dyn Gh, worktree: &Path, pr: &NewPr<'_>, cancel: &CancelToken) -> Result<String> {
    let branch = Tool::git(worktree)
        .args(["symbolic-ref", "--short", "--quiet", "HEAD"])
        .query(cancel)
        .map(|s| s.trim().to_string())
        .filter(|b| !b.is_empty())
        .ok_or_else(|| anyhow!("a pull request needs a branch, and {} is on a detached head", worktree.display()))?;
    if pr.title.trim().is_empty() {
        bail!("the pull request title is empty");
    }

    let listed = run_checked(
        gh,
        Some(worktree),
        &["pr", "list", "--head", &branch, "--state", "open", "--json", "url", "--limit", "1"],
        None,
        cancel,
    )?;
    if let Some(url) = existing_url(&listed) {
        return Ok(url);
    }

    let mut args = vec!["pr", "create", "--base", pr.base, "--head", &branch, "--title", pr.title, "--body-file", "-"];
    if pr.draft {
        args.push("--draft");
    }
    let out = gh.run(Some(worktree), &strings(&args), Some(pr.body), cancel)?;
    let found = |text: &str| crate::pr::find_urls(text).into_iter().next();
    if out.success {
        return found(&out.stdout)
            .or_else(|| found(&out.stderr))
            .ok_or_else(|| anyhow!("gh printed no pull request URL: {}", tail(&out.stdout, &out.stderr)));
    }
    // Lost a race with another creator: gh names the existing one in its error.
    if out.stderr.contains("already exists")
        && let Some(url) = found(&out.stderr)
    {
        return Ok(url);
    }
    Err(anyhow!("gh pr create failed: {}", tail(&out.stderr, &out.stdout)))
}

/// Reads the state of the pull request at `url`: state, checks, review, size, branches.
/// Needs `gh` signed in.
pub fn view(url: &str, cancel: &CancelToken) -> Result<PullRequest> {
    view_with(&Cli, url, cancel)
}

/// [`view`] with a custom `gh`.
pub fn view_with(gh: &dyn Gh, url: &str, cancel: &CancelToken) -> Result<PullRequest> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') || !url.contains("/pull/") {
        bail!("status is only available for GitHub pull requests (got `{}`)", redact_credentials(url));
    }
    let json = run_checked(gh, None, &["pr", "view", url, "--json", VIEW_FIELDS], None, cancel)?;
    parse_view(&json, url)
}

/// Turns the JSON of `gh pr view --json …` into a [`PullRequest`]. `url` is used when the JSON
/// has none.
pub fn parse_view(json: &str, url: &str) -> Result<PullRequest> {
    let value: Value = serde_json::from_str(json).map_err(|e| anyhow!("gh printed unreadable JSON: {e}"))?;
    let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
    let count = |key: &str| u32::try_from(value[key].as_u64().unwrap_or(0)).unwrap_or(u32::MAX);
    let state = match (value["state"].as_str().unwrap_or_default(), value["isDraft"].as_bool().unwrap_or(false)) {
        ("MERGED", _) => "merged",
        ("CLOSED", _) => "closed",
        (_, true) => "draft",
        _ => "open",
    };
    let review = match value["reviewDecision"].as_str().unwrap_or_default() {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "changes_requested",
        "REVIEW_REQUIRED" => "review_required",
        _ => "",
    };
    Ok(PullRequest {
        url: Some(text("url")).filter(|u| !u.is_empty()).unwrap_or_else(|| url.to_string()),
        number: value["number"].as_u64().unwrap_or(0),
        title: text("title"),
        state: state.to_string(),
        checks: checks_summary(&value["statusCheckRollup"]).to_string(),
        review: review.to_string(),
        added: count("additions"),
        removed: count("deletions"),
        head: text("headRefName"),
        base: text("baseRefName"),
    })
}

/// `failing` when any check failed, else `pending` when any is still running, else `passing`;
/// empty when there are no checks.
fn checks_summary(rollup: &Value) -> &'static str {
    let (mut passed, mut failed, mut pending) = (0, 0, 0);
    for check in rollup.as_array().into_iter().flatten() {
        // Check runs have status and conclusion; commit statuses have a state.
        let field = |key: &str| check[key].as_str().unwrap_or_default();
        match (field("conclusion"), field("state")) {
            ("SUCCESS" | "NEUTRAL" | "SKIPPED", _) | (_, "SUCCESS") => passed += 1,
            ("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE", _)
            | (_, "FAILURE" | "ERROR") => failed += 1,
            _ => pending += 1,
        }
    }
    if failed > 0 {
        "failing"
    } else if pending > 0 {
        "pending"
    } else if passed > 0 {
        "passing"
    } else {
        ""
    }
}

/// Pull request (GitHub) and merge request (GitLab) URLs in free text, each cut down to the
/// request itself (no `/files`, no fragment), in order of appearance, without duplicates.
/// `github.com/o/r/pull/new/branch` (the hint `git push` prints) is not one.
pub fn find_urls(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for (start, _) in text.match_indices("http") {
        let rest = &text[start..];
        if !(rest.starts_with("https://") || rest.starts_with("http://")) {
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\'' | '`' | ',' | '<'))
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(['.', ';', ':']);
        if let Some(url) = canonical(url)
            && !found.contains(&url)
        {
            found.push(url);
        }
    }
    found
}

/// `…/pull/42` or `…/-/merge_requests/7` out of a longer URL.
fn canonical(url: &str) -> Option<String> {
    let url = url.split(['#', '?']).next().unwrap_or(url);
    for marker in ["/pull/", "/-/merge_requests/"] {
        if let Some(i) = url.find(marker) {
            let digits: String = url[i + marker.len()..].chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                return Some(format!("{}{marker}{digits}", &url[..i]));
            }
        }
    }
    None
}

/// The URL of the first pull request in the JSON of `gh pr list --json url`.
fn existing_url(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    value.as_array()?.first()?["url"].as_str().map(str::to_string).filter(|u| !u.is_empty())
}

fn run_checked(gh: &dyn Gh, dir: Option<&Path>, args: &[&str], stdin: Option<&str>, cancel: &CancelToken) -> Result<String> {
    let out = gh.run(dir, &strings(args), stdin, cancel)?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(anyhow!("`gh {}` failed: {}", args.iter().take(2).copied().collect::<Vec<_>>().join(" "), tail(&out.stderr, &out.stdout)))
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

/// The first non-empty of two outputs, trimmed and cut to a readable size.
fn tail(primary: &str, secondary: &str) -> String {
    let text = if primary.trim().is_empty() { secondary.trim() } else { primary.trim() };
    let text = redact_credentials(text);
    if text.chars().count() > 600 {
        let cut: String = text.chars().skip(text.chars().count() - 600).collect();
        format!("…{cut}")
    } else if text.is_empty() {
        "no output".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, git};
    use std::cell::RefCell;

    type Call = (Vec<String>, Option<String>);
    type Answer = Box<dyn Fn(&[String]) -> GhOutput>;

    /// Answers `gh` calls from a script and records them.
    struct Fake {
        calls: RefCell<Vec<Call>>,
        answer: Answer,
    }

    impl Fake {
        fn new(answer: impl Fn(&[String]) -> GhOutput + 'static) -> Self {
            Self { calls: RefCell::new(Vec::new()), answer: Box::new(answer) }
        }
    }

    impl Gh for Fake {
        fn run(&self, _dir: Option<&Path>, args: &[String], stdin: Option<&str>, _c: &CancelToken) -> Result<GhOutput> {
            self.calls.borrow_mut().push((args.to_vec(), stdin.map(str::to_string)));
            Ok((self.answer)(args))
        }
    }

    fn ok(stdout: &str) -> GhOutput {
        GhOutput { success: true, stdout: stdout.into(), stderr: String::new() }
    }

    fn fail(stderr: &str) -> GhOutput {
        GhOutput { success: false, stdout: String::new(), stderr: stderr.into() }
    }

    const CANNED: &str = r#"{
        "url": "https://github.com/acme/api/pull/42",
        "number": 42, "title": "HAR-4: comments", "state": "OPEN", "isDraft": false,
        "reviewDecision": "APPROVED",
        "statusCheckRollup": [
            {"status": "COMPLETED", "conclusion": "SUCCESS"},
            {"status": "COMPLETED", "conclusion": "SKIPPED"},
            {"state": "SUCCESS"}
        ],
        "additions": 120, "deletions": 8, "headRefName": "chat/comments", "baseRefName": "main"
    }"#;

    #[test]
    fn finds_pull_and_merge_request_urls() {
        let text = "Opened https://github.com/acme/api/pull/42. See (https://gitlab.com/g/p/-/merge_requests/7/diffs) \
            and https://github.com/acme/api/pull/42/files#diff-1 and https://github.com/acme/api/issues/3 \
            and [md](https://github.com/acme/web/pull/9) and http://gitlab.example.org/a/b/-/merge_requests/12?x=1";
        assert_eq!(
            find_urls(text),
            [
                "https://github.com/acme/api/pull/42",
                "https://gitlab.com/g/p/-/merge_requests/7",
                "https://github.com/acme/web/pull/9",
                "http://gitlab.example.org/a/b/-/merge_requests/12",
            ]
        );
        assert!(find_urls("remote: Create a pull request: https://github.com/acme/api/pull/new/chat/x").is_empty());
        assert!(find_urls("no links, just http and https://").is_empty());
        assert!(find_urls("").is_empty());
    }

    #[test]
    fn view_parses_canned_json() {
        let gh = Fake::new(|_| ok(CANNED));
        let pr = view_with(&gh, "https://github.com/acme/api/pull/42", &CancelToken::new()).expect("view");
        assert_eq!(
            pr,
            PullRequest {
                url: "https://github.com/acme/api/pull/42".into(),
                number: 42,
                title: "HAR-4: comments".into(),
                state: "open".into(),
                checks: "passing".into(),
                review: "approved".into(),
                added: 120,
                removed: 8,
                head: "chat/comments".into(),
                base: "main".into(),
            }
        );
        let calls = gh.calls.borrow();
        assert_eq!(calls[0].0[..3], ["pr", "view", "https://github.com/acme/api/pull/42"]);
        assert!(calls[0].0.contains(&"--json".to_string()));
    }

    #[test]
    fn view_maps_states_checks_and_reviews() {
        let parse = |json: &str| parse_view(json, "https://github.com/a/b/pull/1").expect("parse");
        let pr = parse(r#"{"state":"OPEN","isDraft":true,"reviewDecision":"CHANGES_REQUESTED",
            "statusCheckRollup":[{"conclusion":"SUCCESS"},{"conclusion":"FAILURE"},{"status":"IN_PROGRESS","conclusion":""}]}"#);
        assert_eq!((pr.state.as_str(), pr.checks.as_str(), pr.review.as_str()), ("draft", "failing", "changes_requested"));
        assert_eq!(pr.url, "https://github.com/a/b/pull/1");
        let pr = parse(r#"{"state":"MERGED","isDraft":true,"reviewDecision":"","statusCheckRollup":[]}"#);
        assert_eq!((pr.state.as_str(), pr.checks.as_str(), pr.review.as_str()), ("merged", "", ""));
        let pr = parse(r#"{"state":"CLOSED","reviewDecision":"REVIEW_REQUIRED","statusCheckRollup":[{"status":"QUEUED"},{"state":"ERROR"}]}"#);
        assert_eq!((pr.state.as_str(), pr.checks.as_str(), pr.review.as_str()), ("closed", "failing", "review_required"));
        let pr = parse(r#"{"state":"OPEN","statusCheckRollup":[{"status":"QUEUED"},{"conclusion":"SUCCESS"}]}"#);
        assert_eq!(pr.checks, "pending");
        assert!(parse_view("not json", "u").is_err());
    }

    #[test]
    fn view_rejects_other_urls_and_reports_gh_failures() {
        let gh = Fake::new(|_| fail("gh: not logged in\nrun gh auth login"));
        let e = view_with(&gh, "https://gitlab.com/g/p/-/merge_requests/7", &CancelToken::new()).expect_err("gitlab");
        assert!(e.to_string().contains("only available for GitHub"), "{e}");
        assert!(gh.calls.borrow().is_empty());
        let e = view_with(&gh, "https://github.com/a/b/pull/1", &CancelToken::new()).expect_err("fails");
        assert!(e.to_string().contains("not logged in"), "{e}");
    }

    fn pushed_branch(sandbox: &Sandbox, name: &str) -> std::path::PathBuf {
        let repo = sandbox.repo(name);
        git(&repo, &["checkout", "-q", "-b", "chat/fix"]);
        repo
    }

    #[test]
    fn create_opens_a_pull_request_and_passes_the_body_on_stdin() {
        let sandbox = Sandbox::new();
        let repo = pushed_branch(&sandbox, "create");
        let gh = Fake::new(|args| match args[1].as_str() {
            "list" => ok("[]"),
            _ => ok("Creating pull request for chat/fix into main\nhttps://github.com/acme/api/pull/77\n"),
        });
        let new = NewPr { title: "HAR-4: fix", body: "Body text\n\n## Run", base: "main", draft: true };
        let url = create_with(&gh, &repo, &new, &CancelToken::new()).expect("create");
        assert_eq!(url, "https://github.com/acme/api/pull/77");
        let calls = gh.calls.borrow();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0.windows(2).any(|w| w == ["--head", "chat/fix"]));
        let (args, stdin) = &calls[1];
        assert!(args.windows(2).any(|w| w == ["--base", "main"]));
        assert!(args.windows(2).any(|w| w == ["--title", "HAR-4: fix"]));
        assert!(args.windows(2).any(|w| w == ["--body-file", "-"]));
        assert!(args.contains(&"--draft".to_string()));
        assert_eq!(stdin.as_deref(), Some("Body text\n\n## Run"));
    }

    #[test]
    fn create_returns_the_existing_pull_request() {
        let sandbox = Sandbox::new();
        let repo = pushed_branch(&sandbox, "existing");
        let gh = Fake::new(|args| match args[1].as_str() {
            "list" => ok(r#"[{"url":"https://github.com/acme/api/pull/5"}]"#),
            _ => fail("must not create"),
        });
        let new = NewPr { title: "t", body: "b", base: "main", draft: false };
        assert_eq!(
            create_with(&gh, &repo, &new, &CancelToken::new()).expect("existing"),
            "https://github.com/acme/api/pull/5"
        );
        assert_eq!(gh.calls.borrow().len(), 1);

        // Lost a race: create fails but names the pull request.
        let gh = Fake::new(|args| match args[1].as_str() {
            "list" => ok("[]"),
            _ => fail("a pull request for branch \"chat/fix\" into branch \"main\" already exists:\nhttps://github.com/acme/api/pull/6\n"),
        });
        assert_eq!(
            create_with(&gh, &repo, &new, &CancelToken::new()).expect("raced"),
            "https://github.com/acme/api/pull/6"
        );
    }

    #[test]
    fn create_reports_failures_and_detached_heads() {
        let sandbox = Sandbox::new();
        let repo = pushed_branch(&sandbox, "failing");
        let new = NewPr { title: "t", body: "b", base: "main", draft: false };
        let gh = Fake::new(|args| match args[1].as_str() {
            "list" => ok("[]"),
            _ => fail("GraphQL: Head sha can't be blank"),
        });
        let e = create_with(&gh, &repo, &new, &CancelToken::new()).expect_err("fails");
        assert!(e.to_string().contains("Head sha can't be blank"), "{e}");

        let gh = Fake::new(|args| match args[1].as_str() {
            "list" => ok("[]"),
            _ => ok("done"),
        });
        let e = create_with(&gh, &repo, &new, &CancelToken::new()).expect_err("no url");
        assert!(e.to_string().contains("no pull request URL"), "{e}");

        git(&repo, &["checkout", "-q", "--detach"]);
        let e = create_with(&gh, &repo, &new, &CancelToken::new()).expect_err("detached");
        assert!(e.to_string().contains("detached head"), "{e}");
    }
}
