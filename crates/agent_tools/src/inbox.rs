//! Inbox reports: what an agent hands back when it finishes a task — a title, a Markdown
//! summary, an outcome and links (pull requests first). Agents post them with the
//! `report_to_inbox` tool; pull requests opened during a run without a report are captured
//! automatically. Stored in `$XDG_DATA_HOME/harness/inbox.json`, shown on the Inbox screen.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub const REPORT_TOOL: &str = "report_to_inbox";

/// How the task ended, as the agent reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Finished; nothing needed from the user.
    Done,
    /// Finished, and the user should review (e.g. a PR to approve).
    NeedsReview,
    /// Stopped: the agent needs a decision or access.
    Blocked,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::NeedsReview => "needs_review",
            Outcome::Blocked => "blocked",
            Outcome::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "done" => Outcome::Done,
            "needs_review" | "review" => Outcome::NeedsReview,
            "blocked" => Outcome::Blocked,
            "failed" => Outcome::Failed,
            _ => return None,
        })
    }

    /// Label for badges and list rows.
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::NeedsReview => "needs review",
            Outcome::Blocked => "blocked",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    PullRequest,
    Issue,
    Commit,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub url: String,
    pub label: String,
    pub kind: LinkKind,
}

impl Link {
    pub fn new(url: &str, label: Option<&str>) -> Self {
        let kind = link_kind(url);
        // Pull requests always read `owner/repo#42`; agents tend to label them just "PR".
        let label = label
            .filter(|_| kind != LinkKind::PullRequest)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| short_label(url, kind));
        Self {
            url: url.trim().to_string(),
            label,
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InboxItem {
    pub id: String,
    /// Unix seconds.
    pub created: u64,
    pub chat_id: String,
    pub chat_title: String,
    /// "Build", "Plan" or a custom agent's name.
    pub agent: String,
    pub workdir: Option<PathBuf>,
    /// `repo · branch` of the workdir.
    pub wd_label: String,
    pub title: String,
    /// Markdown.
    pub summary: String,
    pub outcome: Outcome,
    pub links: Vec<Link>,
    pub read: bool,
    pub archived: bool,
    /// Captured by the app (a PR appeared in the run without a report).
    pub auto: bool,
}

impl InboxItem {
    pub fn prs(&self) -> impl Iterator<Item = &Link> {
        self.links.iter().filter(|l| l.kind == LinkKind::PullRequest)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "created": self.created,
            "chat_id": self.chat_id,
            "chat_title": self.chat_title,
            "agent": self.agent,
            "workdir": self.workdir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "wd_label": self.wd_label,
            "title": self.title,
            "summary": self.summary,
            "outcome": self.outcome.as_str(),
            "links": self.links.iter().map(|l| json!({"url": l.url, "label": l.label})).collect::<Vec<_>>(),
            "read": self.read,
            "archived": self.archived,
            "auto": self.auto,
        })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Some(Self {
            id: v["id"].as_str()?.to_string(),
            created: v["created"].as_u64().unwrap_or(0),
            chat_id: s("chat_id"),
            chat_title: s("chat_title"),
            agent: s("agent"),
            workdir: v["workdir"]
                .as_str()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            wd_label: s("wd_label"),
            title: s("title"),
            summary: s("summary"),
            outcome: v["outcome"]
                .as_str()
                .and_then(Outcome::parse)
                .unwrap_or(Outcome::Done),
            links: v["links"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|l| Some(Link::new(l["url"].as_str()?, l["label"].as_str())))
                        .collect()
                })
                .unwrap_or_default(),
            read: v["read"].as_bool().unwrap_or(false),
            archived: v["archived"].as_bool().unwrap_or(false),
            auto: v["auto"].as_bool().unwrap_or(false),
        })
    }
}

// ── Store ───────────────────────────────────────────────────────────────

/// `$XDG_DATA_HOME/harness` (default `~/.local/share/harness`): app data shared by the
/// chat, the Inbox and pipelines.
pub fn data_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("harness")
}

pub fn path() -> PathBuf {
    data_dir().join("inbox.json")
}

/// All items, newest first.
pub fn load() -> Vec<InboxItem> {
    load_from(&path())
}

pub fn load_from(path: &Path) -> Vec<InboxItem> {
    let Some(v) = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return Vec::new();
    };
    let mut items: Vec<InboxItem> = v["items"]
        .as_array()
        .map(|a| a.iter().filter_map(InboxItem::from_json).collect())
        .unwrap_or_default();
    items.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    items
}

pub fn save(items: &[InboxItem]) -> std::io::Result<()> {
    save_to(&path(), items)
}

pub fn save_to(path: &Path, items: &[InboxItem]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let data = serde_json::to_vec_pretty(&json!({
        "items": items.iter().map(InboxItem::to_json).collect::<Vec<_>>()
    }))
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, path)
}

/// Add `item` (newest first) and save.
pub fn add(item: InboxItem) -> std::io::Result<()> {
    let mut items = load();
    items.insert(0, item);
    save(&items)
}

/// Change one item in place and save. `false` when there is no such item.
pub fn update(id: &str, f: impl FnOnce(&mut InboxItem)) -> std::io::Result<bool> {
    let mut items = load();
    let Some(item) = items.iter_mut().find(|i| i.id == id) else {
        return Ok(false);
    };
    f(item);
    save(&items)?;
    Ok(true)
}

/// Unread, not archived items.
pub fn unread(items: &[InboxItem]) -> usize {
    items.iter().filter(|i| !i.read && !i.archived).count()
}

// ── Tool ────────────────────────────────────────────────────────────────

pub fn spec() -> llm::ToolSpec {
    llm::ToolSpec {
        name: REPORT_TOOL.into(),
        description: "Post the result of a finished task to the user's Inbox. Call it once, at \
            the end of a task that produced something (code changes, a pull request, a branch, \
            a plan or an investigation the user asked for) — not for quick answers. Include \
            every pull request, issue, branch or commit URL you created."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "What was done, as a short headline (under 80 characters)" },
                "summary": { "type": "string", "description": "Markdown: what changed, how it was verified, anything left for the user" },
                "status": {
                    "type": "string",
                    "enum": ["done", "needs_review", "blocked", "failed"],
                    "description": "needs_review when the user should review or approve (e.g. an open PR); blocked when you need a decision or access"
                },
                "links": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string" },
                            "label": { "type": "string", "description": "Optional, e.g. \"PR #42\"" }
                        },
                        "required": ["url"]
                    }
                }
            },
            "required": ["title", "summary"]
        }),
    }
}

/// What the agent reported (tool input).
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub title: String,
    pub summary: String,
    pub outcome: Outcome,
    pub links: Vec<Link>,
}

pub fn parse(input: &Value) -> Result<Report, String> {
    if let Value::String(s) = input {
        let v: Value = serde_json::from_str(s).map_err(|e| format!("invalid JSON input: {e}"))?;
        return parse(&v);
    }
    let title = input["title"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("`title` is required")?;
    let summary = input["summary"].as_str().unwrap_or_default().trim();
    let outcome = match input["status"].as_str() {
        None => Outcome::Done,
        Some(s) => Outcome::parse(s).ok_or_else(|| {
            format!("`status` must be done, needs_review, blocked or failed (got `{s}`)")
        })?,
    };
    let mut links: Vec<Link> = Vec::new();
    for l in input["links"].as_array().into_iter().flatten() {
        let url = match l {
            Value::String(u) => u.as_str(),
            _ => l["url"].as_str().unwrap_or_default(),
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!("link `{url}` is not an http(s) URL"));
        }
        if !links.iter().any(|x| x.url == url) {
            links.push(Link::new(url, l["label"].as_str()));
        }
    }
    // PR links the agent only mentioned in the summary count too.
    for url in find_pr_urls(summary) {
        if !links.iter().any(|x| x.url == url) {
            links.push(Link::new(&url, None));
        }
    }
    Ok(Report {
        title: truncate(title, 120),
        summary: summary.to_string(),
        outcome,
        links,
    })
}

// ── Links ───────────────────────────────────────────────────────────────

pub fn link_kind(url: &str) -> LinkKind {
    let u = url.trim_end_matches('/');
    if u.contains("/pull/") || u.contains("/-/merge_requests/") || u.contains("/pull-requests/") {
        LinkKind::PullRequest
    } else if u.contains("/issues/") || u.contains("/-/issues/") {
        LinkKind::Issue
    } else if u.contains("/commit/") || u.contains("/-/commit/") {
        LinkKind::Commit
    } else {
        LinkKind::Other
    }
}

/// `owner/repo#42` for PRs and issues, `owner/repo@abc1234` for commits, else the host+path.
pub fn short_label(url: &str, kind: LinkKind) -> String {
    let rest = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let marker = |m: &str| parts.iter().position(|p| *p == m);
    let repo = |i: usize| {
        parts[1..i]
            .iter()
            .filter(|p| **p != "-")
            .copied()
            .collect::<Vec<_>>()
            .join("/")
    };
    match kind {
        LinkKind::PullRequest | LinkKind::Issue => {
            let i = marker("pull")
                .or_else(|| marker("merge_requests"))
                .or_else(|| marker("pull-requests"))
                .or_else(|| marker("issues"));
            if let Some(i) = i
                && i > 1
                && let Some(n) = parts.get(i + 1)
            {
                return format!("{}#{n}", repo(i));
            }
        }
        LinkKind::Commit => {
            if let Some(i) = marker("commit")
                && i > 1
                && let Some(sha) = parts.get(i + 1)
            {
                return format!("{}@{}", repo(i), &sha[..sha.len().min(7)]);
            }
        }
        LinkKind::Other => {}
    }
    rest.trim_end_matches('/').to_string()
}

/// Pull / merge request URLs in `text` (GitHub, GitLab, Bitbucket), in order, unique.
pub fn find_pr_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (start, _) in text.match_indices("http") {
        let rest = &text[start..];
        if !(rest.starts_with("https://") || rest.starts_with("http://")) {
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\'' | '`' | ',' | '<'))
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(['.', ';', ':']);
        if link_kind(url) != LinkKind::PullRequest {
            continue;
        }
        // Cut sub-pages (`/files`, `/diffs`) and fragments to the PR itself; skip
        // `/pull/new/<branch>` (the "create a PR" hint `git push` prints).
        let Some(url) = canonical_pr(url) else {
            continue;
        };
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

fn canonical_pr(url: &str) -> Option<String> {
    let url = url.split(['#', '?']).next().unwrap_or(url);
    for m in ["/pull/", "/-/merge_requests/", "/pull-requests/"] {
        if let Some(i) = url.find(m) {
            let tail = &url[i + m.len()..];
            let n: String = tail.chars().take_while(char::is_ascii_digit).collect();
            if !n.is_empty() {
                return Some(format!("{}{m}{n}", &url[..i]));
            }
        }
    }
    None
}

/// A tool call that creates a pull / merge request: `gh pr create`, `glab mr create`,
/// `hub pull-request`, or an MCP tool like `create_pull_request`. Only their output is
/// searched for new PRs (listing or viewing PRs must not post anything).
pub fn creates_pr(tool: &str, input: &Value) -> bool {
    if tool == "bash" {
        let cmd = input["command"].as_str().unwrap_or_default();
        let cmd = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
        return ["gh pr create", "glab mr create", "hub pull-request", "gh api repos/"]
            .iter()
            .any(|p| cmd.contains(p))
            && (cmd.contains("pr create") || cmd.contains("mr create") || cmd.contains("pull-request") || cmd.contains("/pulls"));
    }
    let t = tool.to_ascii_lowercase();
    t.contains("create_pull_request") || t.contains("create_merge_request") || t.contains("createpullrequest")
}

// ── Pull request status (gh) ────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct PrInfo {
    pub title: String,
    /// OPEN, MERGED, CLOSED.
    pub state: String,
    pub draft: bool,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED or empty.
    pub review: String,
    pub checks_passed: usize,
    pub checks_failed: usize,
    pub checks_pending: usize,
    pub additions: u64,
    pub deletions: u64,
    pub head: String,
    pub base: String,
}

impl PrInfo {
    /// `open`, `draft`, `merged`, `closed`.
    pub fn state_label(&self) -> &'static str {
        match self.state.as_str() {
            "MERGED" => "merged",
            "CLOSED" => "closed",
            _ if self.draft => "draft",
            _ => "open",
        }
    }
}

/// Status of a GitHub pull request via `gh pr view` (needs `gh` signed in).
pub fn fetch_pr(url: &str) -> Result<PrInfo, String> {
    if !url.contains("github.com/") {
        return Err("status is only available for GitHub pull requests".into());
    }
    let out = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            url,
            "--json",
            "title,state,isDraft,reviewDecision,statusCheckRollup,additions,deletions,headRefName,baseRefName",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("gh not available: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().next().unwrap_or("gh failed").trim().to_string());
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    Ok(pr_info_from_json(&v))
}

pub fn pr_info_from_json(v: &Value) -> PrInfo {
    let (mut passed, mut failed, mut pending) = (0, 0, 0);
    for c in v["statusCheckRollup"].as_array().into_iter().flatten() {
        // Check runs have status/conclusion; commit statuses have state.
        let conclusion = c["conclusion"].as_str().unwrap_or_default();
        let state = c["state"].as_str().unwrap_or_default();
        let status = c["status"].as_str().unwrap_or_default();
        match (conclusion, state, status) {
            ("SUCCESS" | "NEUTRAL" | "SKIPPED", _, _) | (_, "SUCCESS", _) => passed += 1,
            ("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE", _, _)
            | (_, "FAILURE" | "ERROR", _) => failed += 1,
            _ => pending += 1,
        }
    }
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    PrInfo {
        title: s("title"),
        state: s("state"),
        draft: v["isDraft"].as_bool().unwrap_or(false),
        review: s("reviewDecision"),
        checks_passed: passed,
        checks_failed: failed,
        checks_pending: pending,
        additions: v["additions"].as_u64().unwrap_or(0),
        deletions: v["deletions"].as_u64().unwrap_or(0),
        head: s("headRefName"),
        base: s("baseRefName"),
    }
}

pub fn new_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("r{}-{:04}", now.as_millis(), now.subsec_nanos() % 10_000)
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_pr_urls() {
        let text = "Opened https://github.com/acme/api/pull/42. See (https://gitlab.com/g/p/-/merge_requests/7/diffs) \
            and https://github.com/acme/api/pull/42/files and https://github.com/acme/api/issues/3";
        assert_eq!(
            find_pr_urls(text),
            vec![
                "https://github.com/acme/api/pull/42",
                "https://gitlab.com/g/p/-/merge_requests/7"
            ]
        );
    }

    #[test]
    fn labels() {
        assert_eq!(
            short_label("https://github.com/acme/api/pull/42", LinkKind::PullRequest),
            "acme/api#42"
        );
        assert_eq!(
            short_label("https://gitlab.com/g/p/-/merge_requests/7", LinkKind::PullRequest),
            "g/p#7"
        );
        assert_eq!(
            short_label("https://github.com/acme/api/commit/abcdef123456", LinkKind::Commit),
            "acme/api@abcdef1"
        );
        assert_eq!(link_kind("https://github.com/a/b/issues/1"), LinkKind::Issue);
    }

    #[test]
    fn pr_creating_calls() {
        assert!(creates_pr("bash", &json!({"command": "gh pr create --fill"})));
        assert!(creates_pr("bash", &json!({"command": "git push -u origin x && gh  pr create -t T -b B"})));
        assert!(creates_pr("bash", &json!({"command": "glab mr create"})));
        assert!(!creates_pr("bash", &json!({"command": "gh pr list"})));
        assert!(!creates_pr("bash", &json!({"command": "gh pr view 3"})));
        assert!(creates_pr("mcp__github__create_pull_request", &json!({})));
        assert!(!creates_pr("mcp__github__get_pull_request", &json!({})));
        // `git push` hint is not a PR.
        assert!(find_pr_urls("visit https://github.com/a/b/pull/new/feat-x").is_empty());
    }

    #[test]
    fn parses_report() {
        let r = parse(&json!({
            "title": "Fix retry",
            "summary": "Done. PR: https://github.com/a/b/pull/9",
            "status": "needs_review",
            "links": [{"url": "https://github.com/a/b/commit/abc"}]
        }))
        .unwrap();
        assert_eq!(r.outcome, Outcome::NeedsReview);
        assert_eq!(r.links.len(), 2);
        assert_eq!(r.links[1].kind, LinkKind::PullRequest);
        assert!(parse(&json!({"summary": "x"})).is_err());
        assert!(parse(&json!({"title": "x", "status": "meh"})).is_err());
        assert!(parse(&json!({"title": "x", "links": [{"url": "file:///etc"}]})).is_err());
    }

    #[test]
    fn store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("harness-inbox-{}", std::process::id()));
        let path = dir.join("inbox.json");
        let item = InboxItem {
            id: "r1".into(),
            created: 5,
            chat_id: "c".into(),
            chat_title: "Chat".into(),
            agent: "Build".into(),
            workdir: Some("/x".into()),
            wd_label: "x · main".into(),
            title: "T".into(),
            summary: "S".into(),
            outcome: Outcome::Blocked,
            links: vec![Link::new("https://github.com/a/b/pull/1", None)],
            read: false,
            archived: false,
            auto: true,
        };
        save_to(&path, std::slice::from_ref(&item)).unwrap();
        assert_eq!(load_from(&path), vec![item]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn pr_checks() {
        let v = json!({
            "title": "t", "state": "OPEN", "isDraft": false, "reviewDecision": "APPROVED",
            "statusCheckRollup": [
                {"status": "COMPLETED", "conclusion": "SUCCESS"},
                {"status": "COMPLETED", "conclusion": "FAILURE"},
                {"status": "IN_PROGRESS", "conclusion": ""},
                {"state": "SUCCESS"}
            ]
        });
        let p = pr_info_from_json(&v);
        assert_eq!((p.checks_passed, p.checks_failed, p.checks_pending), (2, 1, 1));
        assert_eq!(p.state_label(), "open");
    }
}
