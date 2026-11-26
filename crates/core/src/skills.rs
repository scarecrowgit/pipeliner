//! Skills: folders with a `SKILL.md` (YAML-ish front matter `name:` / `description:`).
//!
//! Besides listing, this module holds the filesystem half of installing skills: input
//! classification, a staging area on disk (`<data dir>/skill-staging/<id>/`, a
//! `manifest.json` plus the fetched files in `src/`) and the copy into a skill folder.
//! Nothing here touches the network or runs anything; fetching lives in the engine.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{expand_tilde, Config};
use crate::model::{new_id, now};

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The `SKILL.md` file.
    pub path: PathBuf,
    /// The configured skill folder it came from, as written in the config (e.g. `~/skills`).
    pub dir_label: String,
}

/// All skills found in the configured folders. Earlier folders win on name clashes.
pub fn list(config: &Config) -> Vec<Skill> {
    let mut out: Vec<Skill> = vec![];
    for label in &config.skill_dirs {
        let dir = expand_tilde(label);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let file = e.path().join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let dir_name = e.file_name().to_string_lossy().to_string();
            let (name, description) = front_matter(&text);
            let name = name.unwrap_or(dir_name);
            if out.iter().any(|s| s.name == name) {
                continue;
            }
            out.push(Skill {
                name,
                description: description.unwrap_or_default(),
                path: file,
                dir_label: label.clone(),
            });
        }
    }
    out
}

/// The body of a skill (front matter stripped), with its base directory noted.
pub fn load(config: &Config, name: &str) -> Option<String> {
    let skill = list(config).into_iter().find(|s| s.name == name)?;
    let text = std::fs::read_to_string(&skill.path).ok()?;
    let body = strip_front_matter(&text);
    let base = skill
        .path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Some(format!("# Skill: {name}\nBase directory: {base}\n\n{body}"))
}

/// `name` and `description` from the front matter of a SKILL.md.
pub fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut desc = None;
    if let Some(rest) = text.strip_prefix("---") {
        for line in rest.lines().skip(1) {
            if line.trim() == "---" {
                break;
            }
            if let Some(v) = line.strip_prefix("name:") {
                name = Some(v.trim().trim_matches('"').to_string());
            } else if let Some(v) = line.strip_prefix("description:") {
                desc = Some(v.trim().trim_matches('"').to_string());
            }
        }
    }
    (name, desc)
}

fn strip_front_matter(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start();
        }
    }
    text
}

/// Rewrites (or adds) `name:` in the front matter of a SKILL.md.
pub fn set_front_matter_name(text: &str, name: &str) -> String {
    if let Some(rest) = text.strip_prefix("---") {
        // Front matter lines, up to the closing `---`.
        let mut lines: Vec<String> = vec![];
        let mut closed = false;
        let mut tail = String::new();
        let mut it = rest.split_inclusive('\n');
        let first = it.next().unwrap_or("");
        for l in it.by_ref() {
            if l.trim() == "---" {
                closed = true;
                break;
            }
            lines.push(l.to_string());
        }
        if closed {
            for l in it {
                tail.push_str(l);
            }
            let mut replaced = false;
            for l in lines.iter_mut() {
                if l.starts_with("name:") {
                    *l = format!("name: {name}\n");
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                lines.insert(0, format!("name: {name}\n"));
            }
            return format!("---{first}{}---\n{tail}", lines.concat());
        }
    }
    format!("---\nname: {name}\n---\n{text}")
}

// ---------------------------------------------------------------------------
// What the user typed
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Link,
    Command,
    Words,
}

const COMMAND_WORDS: [&str; 14] = [
    "npx", "bunx", "pnpm", "pnpx", "npm", "yarn", "git", "curl", "wget", "uvx", "pipx", "pip",
    "sh", "bash",
];

/// A GitHub URL split into its parts. `git_ref` is the first segment after `tree`/`blob`
/// (branch names with slashes are not told apart from a path).
#[derive(Clone, Debug, PartialEq)]
pub struct GithubRef {
    pub owner: String,
    pub repo: String,
    pub git_ref: Option<String>,
    /// Folder inside the repo ("" = whole repo). For `blob/…/SKILL.md` the file name is dropped.
    pub path: String,
    /// The URL was a `/blob/` one (a single file).
    pub blob: bool,
}

/// Parses `github.com/o/r`, `…/tree/<ref>/<path>` and `…/blob/<ref>/<path>`
/// (with or without scheme / `www.`). `None` for other hosts and other GitHub pages.
pub fn parse_github(url: &str) -> Option<GithubRef> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let rest = rest
        .strip_prefix("www.")
        .unwrap_or(rest)
        .strip_prefix("github.com/")?;
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return None;
    }
    let owner = segs[0].to_string();
    let repo = segs[1].trim_end_matches(".git").to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    match segs.get(2).copied() {
        None => Some(GithubRef {
            owner,
            repo,
            git_ref: None,
            path: String::new(),
            blob: false,
        }),
        Some(kind @ ("tree" | "blob")) => {
            let git_ref = segs.get(3)?.to_string();
            let mut path: Vec<&str> = segs[4..].to_vec();
            let blob = kind == "blob";
            if blob {
                path.pop();
            }
            Some(GithubRef {
                owner,
                repo,
                git_ref: Some(git_ref),
                path: path.join("/"),
                blob,
            })
        }
        _ => None,
    }
}

/// What kind of input this is, and the short badge label for it.
pub fn classify(input: &str) -> (SourceKind, &'static str) {
    let s = input.trim();
    if s.is_empty() {
        return (SourceKind::Words, "");
    }
    if s.starts_with("$ ") {
        return (SourceKind::Command, "Command");
    }
    let is_link = s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("github.com/")
        || s.starts_with("www.github.com/");
    if is_link && !s.contains(char::is_whitespace) {
        let bare = s
            .split(['?', '#'])
            .next()
            .unwrap_or(s)
            .trim_end_matches('/');
        let lower = bare.to_lowercase();
        let gh = parse_github(s);
        let label = if gh.as_ref().is_some_and(|g| g.git_ref.is_some()) && !lower.ends_with(".zip")
        {
            "GitHub folder"
        } else if lower.ends_with(".zip") {
            "Zip"
        } else if gh.is_some() {
            "GitHub repo"
        } else if lower.ends_with(".md") {
            "SKILL.md link"
        } else {
            "Link"
        };
        return (SourceKind::Link, label);
    }
    let first = s.split_whitespace().next().unwrap_or("");
    if COMMAND_WORDS.contains(&first) {
        return (SourceKind::Command, "Command");
    }
    (SourceKind::Words, "Request")
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------

/// Skills fetched from one source, waiting for the user to review them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StagedSet {
    pub id: String,
    /// What the user typed (or the agent passed).
    pub source: String,
    pub kind: SourceKind,
    /// Short name: `acme/agent-skills/documents`, or the command.
    pub label: String,
    pub commit: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub chat_title: Option<String>,
    /// `Orchestrator` when an agent staged it.
    #[serde(default)]
    pub actor: Option<String>,
    /// Files and bytes of everything fetched.
    pub files: usize,
    pub bytes: u64,
    pub skills: Vec<StagedSkill>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StagedSkill {
    pub name: String,
    pub description: String,
    /// Folder of the skill relative to the staged `src/` ("" = src itself).
    pub rel_dir: String,
    pub files: usize,
    /// Files that look executable: `*.sh *.py *.js *.ts *.rb`, files with an executable bit,
    /// or anything inside a `scripts/` folder.
    pub scripts: usize,
    /// Display path of the installed skill folder that already has this name.
    pub clash: Option<String>,
}

pub fn staging_root() -> PathBuf {
    crate::paths::data_dir().join("skill-staging")
}

fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        bail!("bad staging id");
    }
    Ok(())
}

/// `<staging>/<id>`.
pub fn staging_dir(id: &str) -> PathBuf {
    staging_root().join(id)
}

/// `<staging>/<id>/src`: the fetched files ("View files" shows these).
pub fn staged_src(id: &str) -> PathBuf {
    staging_dir(id).join("src")
}

/// Creates an empty staging folder with a fresh id and its `src/`.
pub fn begin_staging() -> Result<(String, PathBuf)> {
    let id = new_id();
    let src = staged_src(&id);
    fs::create_dir_all(&src).with_context(|| format!("creating {}", src.display()))?;
    Ok((id, src))
}

/// Finds every `SKILL.md` below `root` (up to 6 levels deep, not looking inside `.git`,
/// `node_modules` or inside a skill) and returns `(name, description, rel_dir)`.
/// Name: front matter, else the folder name. Symlinked folders are followed.
pub fn scan_dir(root: &Path) -> Vec<(String, String, String)> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, String, String)>) {
        let file = dir.join("SKILL.md");
        if file.is_file() {
            if let Ok(text) = fs::read_to_string(&file) {
                let (name, desc) = front_matter(&text);
                let folder = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let name = name.filter(|n| !n.is_empty()).unwrap_or(folder);
                let rel = dir
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                out.push((name, desc.unwrap_or_default(), rel));
                return;
            }
        }
        if depth >= 6 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        let mut subs: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                let n = e.file_name();
                n != ".git" && n != "node_modules"
            })
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subs.sort();
        for s in subs {
            walk(root, &s, depth + 1, out);
        }
    }
    let mut out = vec![];
    walk(root, root, 0, &mut out);
    out
}

fn is_executable(meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

/// Regular files below `dir` as `(relative path, metadata)`; skips `.git` and symlinks.
fn files_below(dir: &Path) -> Vec<(PathBuf, fs::Metadata)> {
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, fs::Metadata)>) {
        if depth > 30 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.is_dir() {
                if e.file_name() != ".git" {
                    walk(base, &p, depth + 1, out);
                }
            } else if meta.is_file() {
                out.push((p.strip_prefix(base).unwrap_or(&p).to_path_buf(), meta));
            }
        }
    }
    let mut out = vec![];
    walk(dir, dir, 0, &mut out);
    out
}

fn is_script(rel: &Path, meta: &fs::Metadata) -> bool {
    if rel.file_name().is_some_and(|n| n == "SKILL.md") {
        return false;
    }
    let ext_hit = rel
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["sh", "py", "js", "ts", "rb"].contains(&e.to_lowercase().as_str()));
    let in_scripts = rel
        .parent()
        .is_some_and(|p| p.components().any(|c| c.as_os_str() == "scripts"));
    ext_hit || in_scripts || is_executable(meta)
}

/// `~/…` for paths below the home folder.
pub fn display_path(p: &Path) -> String {
    if let Some(h) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&h) {
            return format!("~/{}", rest.display());
        }
    }
    p.display().to_string()
}

/// Describes the skills found in `src`. `clash` is filled from the installed skills.
/// Skills with the same name are listed once (the first found).
pub fn build_staged(
    config: &Config,
    id: &str,
    source: &str,
    kind: SourceKind,
    label: &str,
    commit: Option<String>,
    src: &Path,
) -> StagedSet {
    let all = files_below(src);
    let installed = list(config);
    let mut skills: Vec<StagedSkill> = vec![];
    for (name, description, rel_dir) in scan_dir(src) {
        if skills.iter().any(|s| s.name == name) {
            continue;
        }
        let base = if rel_dir.is_empty() {
            src.to_path_buf()
        } else {
            src.join(&rel_dir)
        };
        let files = files_below(&base);
        let scripts = files.iter().filter(|(r, m)| is_script(r, m)).count();
        skills.push(StagedSkill {
            clash: clash_of(&installed, &name),
            name,
            description,
            rel_dir,
            files: files.len(),
            scripts,
        });
    }
    StagedSet {
        id: id.to_string(),
        source: source.to_string(),
        kind,
        label: label.to_string(),
        commit,
        created_at: now(),
        chat_id: None,
        chat_title: None,
        actor: None,
        files: all.len(),
        bytes: all.iter().map(|(_, m)| m.len()).sum(),
        skills,
    }
}

fn clash_of(installed: &[Skill], name: &str) -> Option<String> {
    installed
        .iter()
        .find(|s| s.name == name)
        .map(|s| display_path(s.path.parent().unwrap_or(&s.path)))
}

/// Writes `manifest.json` of a staged set (the staging folder must exist).
pub fn save_staged(set: &StagedSet) -> Result<()> {
    check_id(&set.id)?;
    let dir = staging_dir(&set.id);
    fs::create_dir_all(&dir)?;
    let tmp = dir.join("manifest.json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(set)?)?;
    fs::rename(&tmp, dir.join("manifest.json"))?;
    Ok(())
}

/// One staged set by id, with `clash` recomputed.
pub fn staged_set(config: &Config, id: &str) -> Option<StagedSet> {
    check_id(id).ok()?;
    let text = fs::read_to_string(staging_dir(id).join("manifest.json")).ok()?;
    let mut set: StagedSet = serde_json::from_str(&text).ok()?;
    let installed = list(config);
    for s in &mut set.skills {
        s.clash = clash_of(&installed, &s.name);
    }
    Some(set)
}

/// All staged sets, newest first. `clash` is recomputed against the skills installed now.
pub fn staged(config: &Config) -> Vec<StagedSet> {
    let Ok(rd) = fs::read_dir(staging_root()) else {
        return vec![];
    };
    let mut out: Vec<StagedSet> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| staged_set(config, &e.file_name().to_string_lossy()))
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    out
}

/// Deletes a staging folder.
pub fn discard_staged(id: &str) -> Result<()> {
    check_id(id)?;
    let dir = staging_dir(id);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Installing
// ---------------------------------------------------------------------------

/// What to do with a staged skill whose name is already installed.
#[derive(Clone, Debug, PartialEq)]
pub enum Clash {
    /// Don't install this one.
    Skip,
    /// Remove the installed folder and install this one in its place.
    Replace,
    /// Install under another name (the `name:` in its SKILL.md is rewritten).
    Rename(String),
}

/// Where new skills go: the first configured skill folder.
pub fn install_dir(config: &Config) -> PathBuf {
    config
        .skill_dirs
        .first()
        .map(|d| expand_tilde(d))
        .unwrap_or_else(|| crate::paths::config_dir().join("skills"))
}

/// Written next to an installed skill: where it came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub kind: SourceKind,
    pub label: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub installed_at: DateTime<Utc>,
    #[serde(default)]
    pub chat_title: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

pub const PROVENANCE_FILE: &str = ".pipeliner-source.json";

/// Where a skill was installed from, if it was installed through Pipeliner.
pub fn provenance(skill: &Skill) -> Option<Provenance> {
    let text = fs::read_to_string(skill.path.parent()?.join(PROVENANCE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn check_name(name: &str) -> Result<()> {
    let bad = name.trim().is_empty()
        || name.starts_with('.')
        || name.contains(['/', '\\', '\0'])
        || name.contains("..");
    if bad {
        bail!("`{name}` is not a usable skill name");
    }
    Ok(())
}

fn check_rel(rel: &str) -> Result<()> {
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("bad skill folder `{rel}`");
    }
    Ok(())
}

/// Copies a folder tree. `.git` is skipped. Symlinks are skipped, unless `follow_within`
/// is set and the link points inside that folder.
pub fn copy_tree(from: &Path, to: &Path, follow_within: Option<&Path>) -> Result<()> {
    fn go(from: &Path, to: &Path, follow: Option<&Path>, depth: usize) -> Result<()> {
        if depth > 30 {
            return Ok(());
        }
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            if e.file_name() == ".git" {
                continue;
            }
            let src = e.path();
            let dst = to.join(e.file_name());
            let lmeta = fs::symlink_metadata(&src)?;
            let meta = if lmeta.file_type().is_symlink() {
                let Some(root) = follow else { continue };
                match fs::canonicalize(&src) {
                    Ok(t) if t.starts_with(root) => fs::metadata(&t)?,
                    _ => continue,
                }
            } else {
                lmeta
            };
            if meta.is_dir() {
                go(&src, &dst, follow, depth + 1)?;
            } else if meta.is_file() {
                fs::copy(&src, &dst).with_context(|| format!("copying {}", src.display()))?;
            }
        }
        Ok(())
    }
    let root = follow_within.and_then(|r| fs::canonicalize(r).ok());
    go(from, to, root.as_deref(), 0)
}

/// Removes an installed skill folder, only if it sits directly in a configured skill folder.
fn remove_installed(config: &Config, folder: &Path) -> Result<()> {
    let parent = folder.parent().context("skill folder has no parent")?;
    let parent = fs::canonicalize(parent)?;
    let allowed = config
        .skill_dirs
        .iter()
        .filter_map(|d| fs::canonicalize(expand_tilde(d)).ok())
        .any(|d| d == parent);
    if !allowed {
        bail!(
            "{} is not inside a configured skill folder; not replacing it",
            folder.display()
        );
    }
    let meta = fs::symlink_metadata(folder)?;
    if meta.file_type().is_symlink() {
        fs::remove_file(folder)?;
    } else {
        fs::remove_dir_all(folder)?;
    }
    Ok(())
}

struct Planned {
    name: String,
    src: PathBuf,
    target: PathBuf,
    rename: Option<String>,
    replace: Option<PathBuf>,
}

/// Installs the picked skills of a staged set into `into` and deletes the staging folder.
/// `picks` are `(skill name, what to do if that name is already installed)`; `Skip` leaves
/// the skill out, `Replace` and `Rename` also work for skills without a clash (they install
/// normally; `Rename` always uses the new name). Everything is checked before anything is
/// copied. Returns the installed folders. Nothing is executed.
pub fn install_staged(
    config: &Config,
    id: &str,
    picks: &[(String, Clash)],
    into: &Path,
) -> Result<Vec<PathBuf>> {
    let set = staged_set(config, id).context("these staged skills are gone")?;
    let src_root = staged_src(id);
    let installed = list(config);

    let mut plan: Vec<Planned> = vec![];
    for (name, how) in picks {
        if *how == Clash::Skip {
            continue;
        }
        let sk = set
            .skills
            .iter()
            .find(|s| &s.name == name)
            .ok_or_else(|| anyhow!("no staged skill named `{name}`"))?;
        check_rel(&sk.rel_dir)?;
        let final_name = match how {
            Clash::Rename(n) => n.trim().to_string(),
            _ => sk.name.clone(),
        };
        check_name(&final_name)?;
        if plan.iter().any(|p| {
            p.target
                .file_name()
                .is_some_and(|f| f == final_name.as_str())
        }) {
            bail!("two skills would be installed as `{final_name}`");
        }
        let existing = installed.iter().find(|s| s.name == final_name);
        let target = into.join(&final_name);
        let mut replace = None;
        match (existing, how) {
            (Some(ex), Clash::Replace) => {
                replace = ex.path.parent().map(|p| p.to_path_buf());
            }
            (Some(_), _) => {
                bail!("`{final_name}` is already installed; pick Replace or another name")
            }
            (None, _) => {}
        }
        // A folder with that name that isn't a listed skill (or lives elsewhere) is in the way.
        let in_the_way = fs::symlink_metadata(&target).is_ok()
            && replace.as_ref().map(|r| r != &target).unwrap_or(true);
        if in_the_way {
            bail!("{} already exists", display_path(&target));
        }
        plan.push(Planned {
            name: sk.name.clone(),
            src: if sk.rel_dir.is_empty() {
                src_root.clone()
            } else {
                src_root.join(&sk.rel_dir)
            },
            target,
            rename: match how {
                Clash::Rename(_) => Some(final_name),
                _ => None,
            },
            replace,
        });
    }

    fs::create_dir_all(into)?;
    let mut out = vec![];
    for p in plan {
        if let Some(old) = &p.replace {
            remove_installed(config, old)?;
        }
        copy_tree(&p.src, &p.target, None)?;
        if let Some(new) = &p.rename {
            let file = p.target.join("SKILL.md");
            let text = fs::read_to_string(&file)?;
            fs::write(&file, set_front_matter_name(&text, new))?;
        }
        let prov = Provenance {
            source: set.source.clone(),
            kind: set.kind,
            label: set.label.clone(),
            commit: set.commit.clone(),
            installed_at: now(),
            chat_title: set.chat_title.clone(),
            actor: set.actor.clone(),
        };
        fs::write(
            p.target.join(PROVENANCE_FILE),
            serde_json::to_string_pretty(&prov)?,
        )
        .with_context(|| format!("writing provenance of {}", p.name))?;
        out.push(p.target);
    }
    discard_staged(id)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pl-skills-{tag}-{}", new_id()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn skill(dir: &Path, folder: &str, name: &str, desc: &str) {
        let d = dir.join(folder);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\nbody of {name}\n"),
        )
        .unwrap();
    }

    fn config_for(dirs: &[&Path]) -> Config {
        Config {
            skill_dirs: dirs.iter().map(|d| d.display().to_string()).collect(),
            ..Config::default()
        }
    }

    // Staging lives under the data dir; tests that use it share one process-wide folder.
    fn use_data_dir() -> PathBuf {
        static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        let d = DIR.get_or_init(|| {
            let d = tmp("data");
            std::env::set_var("PIPELINER_DATA_DIR", &d);
            d
        });
        d.clone()
    }

    #[test]
    fn parses_front_matter() {
        let t = "---\nname: x\ndescription: does y\n---\nbody";
        assert_eq!(front_matter(t), (Some("x".into()), Some("does y".into())));
        assert_eq!(strip_front_matter(t), "body");
    }

    #[test]
    fn classifies_input() {
        let c = |s: &str| classify(s);
        assert_eq!(
            c("https://github.com/acme/agent-skills"),
            (SourceKind::Link, "GitHub repo")
        );
        assert_eq!(
            c("github.com/acme/agent-skills/"),
            (SourceKind::Link, "GitHub repo")
        );
        assert_eq!(
            c("https://github.com/acme/agent-skills/tree/main/documents"),
            (SourceKind::Link, "GitHub folder")
        );
        assert_eq!(
            c("https://github.com/acme/agent-skills/blob/main/x/SKILL.md"),
            (SourceKind::Link, "GitHub folder")
        );
        assert_eq!(
            c("https://example.com/skills.zip"),
            (SourceKind::Link, "Zip")
        );
        assert_eq!(
            c("https://example.com/a/SKILL.md"),
            (SourceKind::Link, "SKILL.md link")
        );
        assert_eq!(c("https://example.com/skills"), (SourceKind::Link, "Link"));
        assert_eq!(
            c("  npx skills add acme/x "),
            (SourceKind::Command, "Command")
        );
        assert_eq!(c("uvx something"), (SourceKind::Command, "Command"));
        assert_eq!(c("$ ./install.sh"), (SourceKind::Command, "Command"));
        assert_eq!(
            c("a skill for release notes"),
            (SourceKind::Words, "Request")
        );
        assert_eq!(c("git is great"), (SourceKind::Command, "Command"));
        assert_eq!(c("nobody npx"), (SourceKind::Words, "Request"));
        assert_eq!(c("  "), (SourceKind::Words, ""));
    }

    #[test]
    fn parses_github_urls() {
        let g = parse_github("https://github.com/acme/skills.git").unwrap();
        assert_eq!(
            (
                g.owner.as_str(),
                g.repo.as_str(),
                g.git_ref,
                g.path.as_str()
            ),
            ("acme", "skills", None, "")
        );
        let g = parse_github("github.com/acme/skills/tree/dev/a/b?x=1").unwrap();
        assert_eq!(g.git_ref.as_deref(), Some("dev"));
        assert_eq!(g.path, "a/b");
        let g = parse_github("https://github.com/acme/skills/blob/main/a/SKILL.md").unwrap();
        assert!(g.blob);
        assert_eq!(g.path, "a");
        assert!(parse_github("https://github.com/acme").is_none());
        assert!(parse_github("https://gitlab.com/a/b").is_none());
        assert!(parse_github("https://github.com/a/b/issues/3").is_none());
    }

    #[test]
    fn scans_nested_skills() {
        let d = tmp("scan");
        skill(&d, "one", "one", "first");
        skill(&d, "group/two", "", "no name");
        skill(&d, "a/b/c/d/e/f", "deep", "within depth");
        skill(&d, "a/b/c/d/e/f/g/h", "too-deep", "beyond depth");
        skill(&d, "node_modules/x", "npm", "skipped");
        skill(&d, ".git/x", "git", "skipped");
        skill(&d, "one/inner", "inner", "inside a skill");
        let found = scan_dir(&d);
        let names: Vec<_> = found.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, vec!["deep", "two", "one"]);
        assert_eq!(found[2].1, "first");
        assert_eq!(found[1].2, "group/two");
        // A skill at the root.
        let r = tmp("scan-root");
        fs::write(r.join("SKILL.md"), "no front matter").unwrap();
        let found = scan_dir(&r);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].2, "");
    }

    #[test]
    fn rewrites_front_matter_name() {
        assert_eq!(
            set_front_matter_name("---\nname: a\ndescription: d\n---\nbody\n", "b"),
            "---\nname: b\ndescription: d\n---\nbody\n"
        );
        assert_eq!(
            set_front_matter_name("---\ndescription: d\n---\nbody", "b"),
            "---\nname: b\ndescription: d\n---\nbody"
        );
        assert_eq!(
            set_front_matter_name("plain", "b"),
            "---\nname: b\n---\nplain"
        );
    }

    #[test]
    fn stage_clash_install_rename_replace_provenance() {
        use_data_dir();
        let mine = tmp("mine");
        skill(&mine, "brand-voice", "brand-voice", "mine");
        fs::write(mine.join("brand-voice/extra.txt"), "old").unwrap();
        let cfg = config_for(&[&mine]);
        assert_eq!(list(&cfg)[0].dir_label, mine.display().to_string());

        let (id, src) = begin_staging().unwrap();
        skill(&src, "pdf-tools", "pdf-tools", "pdf");
        skill(&src, "spreadsheets", "spreadsheets", "xlsx");
        fs::create_dir_all(src.join("spreadsheets/scripts")).unwrap();
        fs::write(src.join("spreadsheets/scripts/fill.py"), "print(1)").unwrap();
        fs::write(src.join("spreadsheets/notes.txt"), "n").unwrap();
        skill(&src, "brand-voice", "brand-voice", "theirs");
        fs::write(src.join("brand-voice/theirs.txt"), "new").unwrap();
        let mut set = build_staged(
            &cfg,
            &id,
            "https://github.com/acme/agent-skills/tree/main/documents",
            SourceKind::Link,
            "acme/agent-skills/documents",
            Some("3f9c2a1".into()),
            &src,
        );
        set.actor = Some("Orchestrator".into());
        set.chat_title = Some("skills for the changelog".into());
        save_staged(&set).unwrap();

        let by = |n: &str| set.skills.iter().find(|s| s.name == n).unwrap().clone();
        assert_eq!(set.skills.len(), 3);
        assert_eq!(by("spreadsheets").scripts, 1);
        assert_eq!(by("spreadsheets").files, 3);
        assert_eq!(by("pdf-tools").scripts, 0);
        assert!(by("pdf-tools").clash.is_none());
        assert!(by("brand-voice")
            .clash
            .as_ref()
            .unwrap()
            .ends_with("brand-voice"));
        assert_eq!(set.files, 6);

        let listed = staged(&cfg);
        assert!(listed.iter().any(|s| s.id == id));
        assert_eq!(staged_set(&cfg, &id).unwrap().skills.len(), 3);

        // Replace works for the clash and for a plain new skill.
        let err = install_staged(
            &cfg,
            &id,
            &[
                ("pdf-tools".into(), Clash::Replace),
                ("brand-voice".into(), Clash::Replace),
            ],
            &mine,
        );
        assert_eq!(err.unwrap().len(), 2);
        assert_eq!(
            fs::read_to_string(mine.join("brand-voice/theirs.txt")).unwrap(),
            "new"
        );
        assert!(!mine.join("brand-voice/extra.txt").exists());
        let listed = list(&cfg);
        let bv = listed.iter().find(|s| s.name == "brand-voice").unwrap();
        let p = provenance(bv).unwrap();
        assert_eq!(p.commit.as_deref(), Some("3f9c2a1"));
        assert_eq!(p.actor.as_deref(), Some("Orchestrator"));
        assert_eq!(p.kind, SourceKind::Link);
        assert!(mine.join("pdf-tools/SKILL.md").is_file());
        // Staging is gone.
        assert!(!staging_dir(&id).exists());
        assert!(staged_set(&cfg, &id).is_none());
    }

    #[test]
    fn install_skip_and_rename_and_conflicts() {
        use_data_dir();
        let mine = tmp("mine2");
        skill(&mine, "brand-voice", "brand-voice", "mine");
        let cfg = config_for(&[&mine]);
        let stage = |cfg: &Config| {
            let (id, src) = begin_staging().unwrap();
            skill(&src, "brand-voice", "brand-voice", "theirs");
            skill(&src, "other", "other", "o");
            let set = build_staged(
                cfg,
                &id,
                "npx skills add acme/x",
                SourceKind::Command,
                "npx skills add acme/x",
                None,
                &src,
            );
            save_staged(&set).unwrap();
            id
        };
        // Skip: nothing installed, staging removed.
        let id = stage(&cfg);
        let out = install_staged(&cfg, &id, &[("brand-voice".into(), Clash::Skip)], &mine).unwrap();
        assert!(out.is_empty());
        assert!(fs::read_to_string(mine.join("brand-voice/SKILL.md"))
            .unwrap()
            .contains("mine"));
        assert!(!staging_dir(&id).exists());

        // A clash with no decision is refused and leaves staging in place.
        let id = stage(&cfg);
        let bad = install_staged(
            &cfg,
            &id,
            &[
                ("other".into(), Clash::Replace),
                ("brand-voice".into(), Clash::Rename("brand-voice".into())),
            ],
            &mine,
        );
        assert!(bad.is_err());
        assert!(!mine.join("other").exists());
        assert!(staging_dir(&id).exists());

        // Rename rewrites name: in the copy.
        let out = install_staged(
            &cfg,
            &id,
            &[
                (
                    "brand-voice".into(),
                    Clash::Rename("brand-voice-acme".into()),
                ),
                ("other".into(), Clash::Replace),
            ],
            &mine,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        let text = fs::read_to_string(mine.join("brand-voice-acme/SKILL.md")).unwrap();
        assert!(text.contains("name: brand-voice-acme") && text.contains("theirs"));
        assert!(fs::read_to_string(mine.join("brand-voice/SKILL.md"))
            .unwrap()
            .contains("mine"));
        let names: Vec<_> = list(&cfg).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["brand-voice", "brand-voice-acme", "other"]);
        let acme = list(&cfg)
            .into_iter()
            .find(|s| s.name == "brand-voice-acme")
            .unwrap();
        assert_eq!(provenance(&acme).unwrap().kind, SourceKind::Command);
        assert!(provenance(&list(&cfg)[0]).is_none());

        // Unsafe names are refused.
        let id = stage(&cfg);
        assert!(install_staged(
            &cfg,
            &id,
            &[("other".into(), Clash::Rename("../x".into()))],
            &mine
        )
        .is_err());
        discard_staged(&id).unwrap();
        assert!(!staging_dir(&id).exists());
        assert!(discard_staged("../etc").is_err());
    }

    #[test]
    fn install_dir_is_first_skill_dir() {
        let a = tmp("a");
        let b = tmp("b");
        assert_eq!(install_dir(&config_for(&[&a, &b])), a);
    }
}
