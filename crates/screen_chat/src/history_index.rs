//! Metadata of every saved chat for the history picker (⌘K): titles, folders, cost,
//! files touched and message snippets, plus the pin / archive flags, search and grouping.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use llm::{ContentBlock, Role};
use serde_json::{Value, json};

use crate::session;
pub use ui::time::{date_of, now_secs, utc_offset, when_label};

/// One message excerpt shown in the picker's preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    /// "You" or the agent's name.
    pub who: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMeta {
    pub id: String,
    pub title: String,
    /// "Build", "Plan" or a custom agent name.
    pub agent: String,
    pub workdir: Option<PathBuf>,
    /// `repo · branch` (or the folder name).
    pub wd_label: String,
    pub cost_usd: f64,
    /// User and assistant messages (tool results not counted).
    pub messages: usize,
    /// Last write of the session file, unix seconds.
    pub updated: u64,
    /// Files written or edited by the agent, in first-touch order.
    pub files: Vec<String>,
    pub snippets: Vec<Snippet>,
    /// Lower-cased title, agent, folder, files and message text, for search.
    hay: String,
}

impl ChatMeta {
    pub fn matches(&self, q_lower: &str) -> bool {
        q_lower.is_empty() || self.hay.contains(q_lower)
    }
}

/// Message text for a snippet: code fences dropped, blank runs collapsed, truncated.
fn snippet_text(t: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in t.lines() {
        let l = line.trim_end();
        if l.trim_start().starts_with("```") || (l.is_empty() && out.last().is_some_and(|p| p.is_empty())) {
            continue;
        }
        out.push(l);
    }
    session::truncate(out.join("\n").trim(), SNIPPET_CHARS)
}

/// Longest snippet kept per message (chars).
const SNIPPET_CHARS: usize = 400;

/// Metadata from a saved session's JSON (`updated` = file mtime).
pub fn meta_from_json(v: &Value, updated: u64) -> Option<ChatMeta> {
    let id = v["id"].as_str()?.to_string();
    let title = v["title"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("Untitled session")
        .to_string();
    let agent = session::agent_label(v);
    let workdir = v["workdir"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let messages: Vec<llm::Message> =
        serde_json::from_value(v["messages"].clone()).unwrap_or_default();
    let mut files: Vec<String> = Vec::new();
    let mut snippets = Vec::new();
    let mut count = 0;
    for m in &messages {
        let mut counted = false;
        for b in &m.content {
            match (m.role, b) {
                (Role::User, ContentBlock::Text(t)) => {
                    if session::decode_file(t).is_some()
                        || session::decode_shell(t).is_some()
                        || session::decode_note(t).is_some()
                        || t.trim().is_empty()
                    {
                        continue;
                    }
                    counted = true;
                    snippets.push(Snippet {
                        who: "You".into(),
                        text: snippet_text(t),
                    });
                }
                (Role::Assistant, ContentBlock::Text(t)) if !t.trim().is_empty() => {
                    counted = true;
                    snippets.push(Snippet {
                        who: agent.clone(),
                        text: snippet_text(t),
                    });
                }
                (Role::Assistant, ContentBlock::ToolUse { name, input, .. }) => {
                    counted = true;
                    if matches!(name.as_str(), "edit_file" | "write_file")
                        && let Some(p) = input["path"].as_str()
                        && !files.iter().any(|f| f == p)
                    {
                        files.push(p.to_string());
                    }
                }
                _ => {}
            }
        }
        if counted {
            count += 1;
        }
    }
    let wd_label = workdir.as_deref().map(folder_label).unwrap_or_default();
    let mut hay = format!("{title}\n{agent}\n{wd_label}\n");
    if let Some(w) = &workdir {
        hay.push_str(&w.to_string_lossy());
        hay.push('\n');
    }
    for f in &files {
        hay.push_str(f);
        hay.push('\n');
    }
    for s in &snippets {
        hay.push_str(&s.text);
        hay.push('\n');
    }
    Some(ChatMeta {
        id,
        title,
        agent,
        workdir,
        wd_label,
        cost_usd: v["cost_usd"].as_f64().unwrap_or(0.),
        messages: count,
        updated,
        files,
        snippets,
        hay: hay.to_lowercase(),
    })
}

/// Every saved chat in `dir`, newest first.
pub fn scan(dir: &Path) -> Vec<ChatMeta> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut labels: HashMap<PathBuf, String> = HashMap::new();
    let mut out: Vec<ChatMeta> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".json") && !name.starts_with('.')
        })
        .filter_map(|e| {
            let updated = e
                .metadata()
                .ok()?
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_secs();
            let v: Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            let mut meta = meta_from_json(&v, updated)?;
            // Same folder → same label; git HEAD is read once per folder.
            if let Some(w) = &meta.workdir {
                meta.wd_label = labels
                    .entry(w.clone())
                    .or_insert_with(|| folder_label(w))
                    .clone();
            }
            Some(meta)
        })
        .collect();
    out.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    out
}

/// `repo · branch` for a working folder: harness worktrees
/// (`~/.harness/worktrees/<repo>/<branch>`) name their repo, other folders their own name;
/// the branch comes from git's `HEAD`.
pub fn folder_label(dir: &Path) -> String {
    let repo = dir
        .strip_prefix(agent_tools::workdir::worktrees_base())
        .ok()
        .and_then(|rest| rest.components().next())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| dir.to_string_lossy().into_owned());
    match head_branch(dir) {
        Some(b) => format!("{repo} · {b}"),
        None => repo,
    }
}

/// Branch (or short commit) checked out in the git repo containing `dir`, from the
/// `HEAD` file — no git process.
pub fn head_branch(dir: &Path) -> Option<String> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let dot = d.join(".git");
        let git_dir = if dot.is_dir() {
            Some(dot)
        } else if dot.is_file() {
            // Linked worktree: `.git` is a file with `gitdir: <path>`.
            std::fs::read_to_string(&dot).ok().and_then(|s| {
                let p = PathBuf::from(s.trim().strip_prefix("gitdir:")?.trim());
                Some(if p.is_absolute() { p } else { d.join(p) })
            })
        } else {
            None
        };
        if let Some(g) = git_dir {
            let head = std::fs::read_to_string(g.join("HEAD")).ok()?;
            let head = head.trim();
            return Some(match head.strip_prefix("ref: ") {
                Some(r) => r.strip_prefix("refs/heads/").unwrap_or(r).to_string(),
                None => head.chars().take(7).collect(),
            });
        }
        cur = d.parent();
    }
    None
}

// ── Pin / archive flags ─────────────────────────────────────────────────

/// Pinned and archived chat ids, kept beside the sessions in `chat_flags.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flags {
    pub pinned: BTreeSet<String>,
    pub archived: BTreeSet<String>,
}

impl Flags {
    pub fn path() -> PathBuf {
        session::sessions_dir()
            .parent()
            .map(|p| p.join("chat_flags.json"))
            .unwrap_or_else(|| PathBuf::from("chat_flags.json"))
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        let Some(v) = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            return Self::default();
        };
        let set = |k: &str| {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            pinned: set("pinned"),
            archived: set("archived"),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let data = serde_json::to_vec_pretty(&json!({
            "pinned": self.pinned,
            "archived": self.archived,
        }))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, path)
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.contains(id)
    }

    pub fn is_archived(&self, id: &str) -> bool {
        self.archived.contains(id)
    }
}

// ── Search ──────────────────────────────────────────────────────────────

/// Live status of a chat (only the open one can be running or waiting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveStatus {
    Waiting,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Mine,
    /// Chats started by agents (none yet: every saved chat was started by the user).
    Agents,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Recent,
    Cost,
}

/// Picker filter state. Tokens are `in:<folder>`, `agent:<name>`, `is:<status|pinned>`,
/// `has:files`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub query: String,
    pub tokens: Vec<String>,
    pub scope: Scope,
    pub waiting: bool,
    pub running: bool,
    pub failed: bool,
    pub has_files: bool,
    pub sort: Sort,
}

impl Filter {
    /// Anything narrowing the list beyond the scope.
    pub fn is_filtering(&self) -> bool {
        !self.query.trim().is_empty()
            || !self.tokens.is_empty()
            || self.waiting
            || self.running
            || self.failed
            || self.has_files
    }
}

/// A recognised `key:value` token at the end of `text` followed by a space
/// (`"fix in:pipeliner "` → `Some(("fix", "in:pipeliner"))`).
pub fn split_trailing_token(text: &str) -> Option<(String, String)> {
    let body = text.strip_suffix(' ')?;
    let start = body.rfind(' ').map_or(0, |i| i + 1);
    let word = &body[start..];
    let (key, value) = word.split_once(':')?;
    if value.is_empty() || !matches!(key, "in" | "agent" | "is" | "has") {
        return None;
    }
    Some((body[..start].trim_end().to_string(), word.to_string()))
}

/// Chats passing `f`, sorted. `status` gives each chat's live status.
pub fn filter<'a>(
    chats: &'a [ChatMeta],
    flags: &Flags,
    f: &Filter,
    status: &dyn Fn(&ChatMeta) -> LiveStatus,
) -> Vec<&'a ChatMeta> {
    let q = f.query.trim().to_lowercase();
    let mut out: Vec<&ChatMeta> = chats
        .iter()
        .filter(|c| !flags.is_archived(&c.id))
        .filter(|_| f.scope != Scope::Agents)
        .filter(|c| f.tokens.iter().all(|t| token_matches(t, c, flags, status(c))))
        .filter(|c| {
            let st = status(c);
            (!f.waiting || st == LiveStatus::Waiting)
                && (!f.running || st == LiveStatus::Running)
                && (!f.failed || st == LiveStatus::Failed)
        })
        .filter(|c| !f.has_files || !c.files.is_empty())
        .filter(|c| c.matches(&q))
        .collect();
    if f.sort == Sort::Cost {
        out.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd));
    }
    out
}

fn token_matches(token: &str, c: &ChatMeta, flags: &Flags, st: LiveStatus) -> bool {
    let Some((key, value)) = token.split_once(':') else {
        return true;
    };
    let value = value.to_lowercase();
    match key {
        "in" => {
            c.wd_label.to_lowercase().starts_with(&value)
                || c.workdir
                    .as_deref()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with(&value))
        }
        "agent" => c.agent.to_lowercase().starts_with(&value),
        "is" => match value.as_str() {
            "waiting" => st == LiveStatus::Waiting,
            "running" => st == LiveStatus::Running,
            "failed" => st == LiveStatus::Failed,
            "done" => st == LiveStatus::Done,
            "pinned" => flags.is_pinned(&c.id),
            _ => false,
        },
        "has" => match value.as_str() {
            "files" => !c.files.is_empty(),
            _ => false,
        },
        _ => true,
    }
}

/// `(start, end)` byte range of the first case-insensitive match of `q` in `text`.
pub fn find_ci(text: &str, q: &str) -> Option<(usize, usize)> {
    if q.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    // Lower-casing can change byte lengths (rare); only trust same-length mappings.
    if lower.len() != text.len() {
        return None;
    }
    let i = lower.find(&q.to_lowercase())?;
    let end = i + q.len();
    (text.is_char_boundary(i) && text.is_char_boundary(end)).then_some((i, end))
}

// ── Time ────────────────────────────────────────────────────────────────

/// History groups, newest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Pinned,
    Today,
    Yesterday,
    ThisWeek,
    ThisMonth,
    /// Earlier, by calendar month: `(year, month 1-12)`.
    Month(i32, u32),
}

impl Group {
    pub fn name(self) -> String {
        match self {
            Group::Pinned => "Pinned".into(),
            Group::Today => "Today".into(),
            Group::Yesterday => "Yesterday".into(),
            Group::ThisWeek => "Earlier this week".into(),
            Group::ThisMonth => "Earlier this month".into(),
            Group::Month(y, m) => format!("{} {y}", ui::time::month_name(m)),
        }
    }

    /// Month groups start folded, like the mockup's older months.
    pub fn folded_by_default(self) -> bool {
        matches!(self, Group::Month(..))
    }
}

/// Which time group a chat last active at `t` falls in, seen at `now`.
pub fn group_of(t: u64, now: u64, offset: i64) -> Group {
    let d = date_of(t, offset);
    let today = date_of(now, offset);
    let ago = today.days - d.days;
    if ago <= 0 {
        Group::Today
    } else if ago == 1 {
        Group::Yesterday
    } else if ago <= i64::from(today.weekday) {
        Group::ThisWeek
    } else if d.year == today.year && d.month == today.month {
        Group::ThisMonth
    } else {
        Group::Month(d.year, d.month)
    }
}

/// Markdown export of chats (title, folder, then each message).
pub fn export_markdown(chats: &[&ChatMeta], dir: &Path) -> String {
    let mut out = String::from("# Harness chats\n");
    for c in chats {
        out.push_str(&format!("\n## {}\n\n", c.title));
        out.push_str(&format!(
            "- Agent: {}\n- Folder: {}\n- Cost: ${:.2}\n\n",
            c.agent,
            c.workdir
                .as_deref()
                .map(|w| w.display().to_string())
                .unwrap_or_default(),
            c.cost_usd
        ));
        if let Some(saved) = session::load(dir, &c.id) {
            for m in &saved.messages {
                for b in &m.content {
                    if let ContentBlock::Text(t) = b {
                        if session::decode_file(t).is_some()
                            || session::decode_shell(t).is_some()
                            || session::decode_note(t).is_some()
                        {
                            continue;
                        }
                        let who = match m.role {
                            Role::User => "You",
                            Role::Assistant => c.agent.as_str(),
                        };
                        out.push_str(&format!("**{who}:**\n\n{}\n\n", t.trim()));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "history_index_tests.rs"]
mod tests;
