//! Fetching skills into the staging area (see `harness_core::skills`): from a link (GitHub
//! repo or folder, `.zip`, raw `SKILL.md`) or by running an install command in an empty
//! temp folder. Nothing is installed here and no fetched file is ever executed; the user
//! reviews the staged set and installs it from Settings › Skills.
//!
//! All functions block (git, downloads, a shell command): call them from a background task.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use harness_core::skills::{self, GithubRef, SourceKind, StagedSet};
use harness_core::Config;

/// Largest download (zip or SKILL.md).
pub const MAX_DOWNLOAD: u64 = 50 * 1024 * 1024;
/// How long an install command may run.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Who asked for the skills, when an agent staged them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Origin {
    pub chat_id: Option<String>,
    pub chat_title: Option<String>,
    pub actor: Option<String>,
}

/// What a link points at, decided from the text alone.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkPlan {
    /// `git clone --depth 1 [--branch <ref>] <clone_url>`, then only `path` is used.
    Git {
        clone_url: String,
        git_ref: Option<String>,
        path: String,
        label: String,
    },
    Zip {
        url: String,
        label: String,
    },
    /// A single SKILL.md.
    Markdown {
        url: String,
        label: String,
    },
    /// Anything else: fetched, and accepted only if it looks like a SKILL.md.
    Other {
        url: String,
        label: String,
    },
}

/// Decides how to fetch a link. Errors for text that is not an http(s) / github.com link.
pub fn plan_link(input: &str) -> Result<LinkPlan> {
    let s = input.trim();
    let url = if s.starts_with("github.com/") || s.starts_with("www.github.com/") {
        format!("https://{s}")
    } else {
        s.to_string()
    };
    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.contains(char::is_whitespace)
    {
        bail!("`{s}` is not a link (expected https://…)");
    }
    if let Some(GithubRef {
        owner,
        repo,
        git_ref,
        path,
        ..
    }) = skills::parse_github(&url)
    {
        let label = if path.is_empty() {
            format!("{owner}/{repo}")
        } else {
            format!("{owner}/{repo}/{path}")
        };
        return Ok(LinkPlan::Git {
            clone_url: format!("https://github.com/{owner}/{repo}"),
            git_ref,
            path,
            label,
        });
    }
    let bare = url.split(['?', '#']).next().unwrap_or(&url).to_lowercase();
    let label = short_url(&url);
    Ok(if bare.ends_with(".zip") {
        LinkPlan::Zip { url, label }
    } else if bare.ends_with(".md") {
        LinkPlan::Markdown { url, label }
    } else {
        LinkPlan::Other { url, label }
    })
}

/// `host/last/segments` without scheme, query and fragment.
fn short_url(url: &str) -> String {
    let rest = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_end_matches('/');
    let mut parts = rest.split('/');
    let host = parts.next().unwrap_or("");
    let tail: Vec<&str> = parts.collect();
    match tail.len() {
        0 => host.to_string(),
        1..=2 => format!("{host}/{}", tail.join("/")),
        n => format!("{host}/…/{}", tail[n - 1]),
    }
}

/// Stages skills from a link. Errors with "found no SKILL.md in …" if there are none.
pub fn stage_link(config: &Config, url: &str, origin: Option<&Origin>) -> Result<StagedSet> {
    let plan = plan_link(url)?;
    let source = url.trim().to_string();
    let (id, src) = skills::begin_staging()?;
    let res = (|| -> Result<StagedSet> {
        let work = skills::staging_dir(&id).join("work");
        std::fs::create_dir_all(&work)?;
        let (label, commit) = match &plan {
            LinkPlan::Git {
                clone_url,
                git_ref,
                path,
                label,
            } => {
                let commit = fetch_git(clone_url, git_ref.as_deref(), path, &work, &src)
                    .with_context(|| format!("fetching {label}"))?;
                (label.clone(), commit)
            }
            LinkPlan::Zip { url, label } => {
                let bytes = download(url)?;
                let zip = work.join("download.zip");
                std::fs::write(&zip, bytes)?;
                let out = work.join("unzipped");
                let r = Command::new("unzip")
                    .args(["-q", "-o"])
                    .arg(&zip)
                    .arg("-d")
                    .arg(&out)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .context("running unzip (is it installed?)")?;
                // unzip exits 1 on warnings only.
                if !r.status.success() && r.status.code() != Some(1) {
                    bail!(
                        "unzip failed: {}",
                        String::from_utf8_lossy(&r.stderr).trim()
                    );
                }
                collect_skills(&out, &src, Some(&out))?;
                (label.clone(), None)
            }
            LinkPlan::Markdown { url, label } => {
                let bytes = download(url)?;
                write_markdown(&src, &bytes)?;
                (label.clone(), None)
            }
            LinkPlan::Other { url, label } => {
                let bytes = download(url)?;
                let text = String::from_utf8_lossy(&bytes);
                if !text
                    .trim_start_matches('\u{feff}')
                    .trim_start()
                    .starts_with("---")
                {
                    bail!("no SKILL.md found at this link");
                }
                write_markdown(&src, &bytes)?;
                (label.clone(), None)
            }
        };
        let _ = std::fs::remove_dir_all(&work);
        finish(
            config,
            &id,
            &source,
            SourceKind::Link,
            &label,
            commit,
            &src,
            origin,
        )
        .with_context(|| format!("found no SKILL.md in {label}"))
    })();
    cleanup_on_error(&id, res)
}

/// Stages skills from any git URL (what `stage_link` does for GitHub).
pub fn stage_git(
    config: &Config,
    clone_url: &str,
    git_ref: Option<&str>,
    path: &str,
    origin: Option<&Origin>,
) -> Result<StagedSet> {
    let (id, src) = skills::begin_staging()?;
    let res = (|| -> Result<StagedSet> {
        let work = skills::staging_dir(&id).join("work");
        std::fs::create_dir_all(&work)?;
        let commit = fetch_git(clone_url, git_ref, path, &work, &src)?;
        let _ = std::fs::remove_dir_all(&work);
        finish(
            config,
            &id,
            clone_url,
            SourceKind::Link,
            clone_url,
            commit,
            &src,
            origin,
        )
    })();
    cleanup_on_error(&id, res)
}

/// Stages skills from an install command: runs it with `sh -c` in an empty temp folder
/// (inside the staging folder) with `HOME` pointing there, so tools that install into
/// `~/.claude/skills` or `./.claude/skills` land in it, then scans what appeared.
/// The command as it will run: `npx skills add …` (and bunx / pnpm dlx / yarn dlx)
/// gets the flags that keep it from asking anything, since it runs without a terminal:
/// it otherwise stops at "which agents?" and fails. `-a claude-code` puts the skill in
/// `.claude/skills` of the temp folder, `--copy` makes it real files (not links into a
/// folder that is deleted), `-y` skips confirmations; npx itself gets `--yes`.
/// Flags you already gave are kept.
pub fn effective_command(command: &str) -> String {
    let cmd = command.trim();
    let cmd = cmd.strip_prefix("$ ").unwrap_or(cmd).trim();
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let runner_len = match words.as_slice() {
        ["npx" | "bunx", ..] => 1,
        ["pnpm" | "yarn", "dlx", ..] => 2,
        _ => return cmd.to_string(),
    };
    // Runner options (`npx -y`), then the package, then `add`.
    let mut i = runner_len;
    while words.get(i).is_some_and(|w| w.starts_with('-')) {
        i += 1;
    }
    let is_skills = words
        .get(i)
        .is_some_and(|p| *p == "skills" || p.starts_with("skills@"));
    if !is_skills || words.get(i + 1) != Some(&"add") || cmd.contains(['|', ';', '&', '>']) {
        return cmd.to_string();
    }
    let has = |names: &[&str]| words[i + 2..].iter().any(|w| names.contains(w));
    let mut out: Vec<String> = words[..runner_len].iter().map(|w| w.to_string()).collect();
    let runner_yes = words[runner_len..i].iter().any(|w| *w == "-y" || *w == "--yes");
    if words[0] == "npx" && !runner_yes {
        out.push("--yes".into());
    }
    out.extend(words[runner_len..].iter().map(|w| w.to_string()));
    if !has(&["-a", "--agent", "--all"]) {
        out.extend(["-a".into(), "claude-code".into()]);
    }
    if !has(&["--copy"]) {
        out.push("--copy".into());
    }
    if !has(&["-y", "--yes", "--all"]) {
        out.push("-y".into());
    }
    out.join(" ")
}

pub fn stage_command(config: &Config, command: &str, origin: Option<&Origin>) -> Result<StagedSet> {
    let effective = effective_command(command);
    let cmd = effective.as_str();
    if cmd.is_empty() {
        bail!("empty command");
    }
    let (id, src) = skills::begin_staging()?;
    let res = (|| -> Result<StagedSet> {
        let work = skills::staging_dir(&id).join("work");
        std::fs::create_dir_all(&work)?;
        let work = work.canonicalize()?;
        let home = work.display().to_string();
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), home.clone());
        env.insert("XDG_CONFIG_HOME".to_string(), format!("{home}/.config"));
        env.insert("XDG_DATA_HOME".to_string(), format!("{home}/.local/share"));
        env.insert("XDG_CACHE_HOME".to_string(), format!("{home}/.cache"));
        env.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());
        // No terminal here: no prompts, colours or spinners.
        for (k, v) in [
            ("CI", "1"),
            ("NO_COLOR", "1"),
            ("FORCE_COLOR", "0"),
            ("TERM", "dumb"),
            ("npm_config_yes", "true"),
        ] {
            env.insert(k.to_string(), v.to_string());
        }
        let cancel = AtomicBool::new(false);
        let output = crate::tools::run_command(&work, "", &env, cmd, COMMAND_TIMEOUT, &cancel)?;
        let ok = output.starts_with("exit code 0\n") || output == "exit code 0";
        if !ok {
            let first = output.lines().next().unwrap_or("failed");
            bail!("`{cmd}` failed ({first})\n{}", tail_of(&output));
        }
        collect_skills(&work, &src, Some(&work)).context("copying what the command installed")?;
        let _ = std::fs::remove_dir_all(&work);
        finish(
            config,
            &id,
            command.trim(),
            SourceKind::Command,
            cmd,
            None,
            &src,
            origin,
        )
        .map_err(|e| anyhow!("{e:#}\n{}", tail_of(&output)))
    })();
    cleanup_on_error(&id, res)
}

fn tail_of(output: &str) -> String {
    let body = output.split_once('\n').map(|x| x.1).unwrap_or("");
    let lines: Vec<&str> = body.lines().collect();
    let from = lines.len().saturating_sub(15);
    lines[from..].join("\n")
}

fn cleanup_on_error(id: &str, res: Result<StagedSet>) -> Result<StagedSet> {
    if res.is_err() {
        let _ = skills::discard_staged(id);
    }
    res
}

/// Builds and saves the manifest. Errors when no skill was found.
#[allow(clippy::too_many_arguments)]
fn finish(
    config: &Config,
    id: &str,
    source: &str,
    kind: SourceKind,
    label: &str,
    commit: Option<String>,
    src: &Path,
    origin: Option<&Origin>,
) -> Result<StagedSet> {
    let mut set = skills::build_staged(config, id, source, kind, label, commit, src);
    if set.skills.is_empty() {
        bail!("found no SKILL.md in {label}");
    }
    if let Some(o) = origin {
        set.chat_id = o.chat_id.clone();
        set.chat_title = o.chat_title.clone();
        set.actor = o.actor.clone();
    }
    skills::save_staged(&set)?;
    Ok(set)
}

/// Shallow clone into `work/repo`, then copy the skills below `path` into `src`.
/// Returns the short commit.
fn fetch_git(
    clone_url: &str,
    git_ref: Option<&str>,
    path: &str,
    work: &Path,
    src: &Path,
) -> Result<Option<String>> {
    if git_ref.is_some_and(|r| r.starts_with('-')) || clone_url.starts_with('-') {
        bail!("bad git ref");
    }
    if Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("bad path `{path}`");
    }
    let mut args = vec!["clone", "--depth", "1", "--quiet"];
    if let Some(r) = git_ref {
        args.extend(["--branch", r]);
    }
    args.extend(["--", clone_url, "repo"]);
    crate::git::run(work, "git", &args)?;
    let repo = work.join("repo");
    let commit = crate::git::git(&repo, &["rev-parse", "--short", "HEAD"]).ok();
    let sub = if path.is_empty() {
        repo.clone()
    } else {
        repo.join(path)
    };
    if !sub.is_dir() {
        bail!("no folder `{path}` in the repository");
    }
    collect_skills(&sub, src, None)?;
    Ok(commit)
}

/// Copies every skill folder found below `root` into `dest`, keeping its relative path.
/// A skill name found twice is copied once. `follow_within`: symlinks inside the skills
/// pointing within this folder are followed (install tools link skills around).
pub fn collect_skills(root: &Path, dest: &Path, follow_within: Option<&Path>) -> Result<()> {
    let mut seen: Vec<String> = vec![];
    for (name, _desc, rel) in skills::scan_dir(root) {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        let from = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let to = if rel.is_empty() {
            dest.to_path_buf()
        } else {
            dest.join(&rel)
        };
        skills::copy_tree(&from, &to, follow_within)?;
    }
    Ok(())
}

/// Writes a downloaded SKILL.md to `src/<name>/SKILL.md`.
fn write_markdown(src: &Path, bytes: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(bytes)
        .trim_start_matches('\u{feff}')
        .to_string();
    let (name, _) = skills::front_matter(&text);
    let folder = folder_name(name.as_deref().unwrap_or("skill"));
    let dir = src.join(folder);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("SKILL.md"), text)?;
    Ok(())
}

fn folder_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches(['-', '.']).to_string();
    if s.is_empty() {
        "skill".into()
    } else {
        s
    }
}

/// GET with a size cap (`MAX_DOWNLOAD`).
fn download(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .user_agent("pipeliner")
        .build()
        .get(url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => anyhow!("{url} answered HTTP {code}"),
            e => anyhow!("could not download {url}: {e}"),
        })?;
    if let Some(len) = resp
        .header("content-length")
        .and_then(|l| l.parse::<u64>().ok())
    {
        if len > MAX_DOWNLOAD {
            bail!(
                "{url} is {} MB, more than the {} MB limit",
                len / 1_048_576,
                MAX_DOWNLOAD / 1_048_576
            );
        }
    }
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut buf)
        .with_context(|| format!("reading {url}"))?;
    if buf.len() as u64 > MAX_DOWNLOAD {
        bail!(
            "{url} is larger than the {} MB limit",
            MAX_DOWNLOAD / 1_048_576
        );
    }
    Ok(buf)
}

/// `PathBuf` helper for callers that show where staged files are.
pub fn staged_files_dir(id: &str) -> PathBuf {
    skills::staged_src(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points the data dir at a temp folder and holds a lock for the test: the tests share
    /// one staging folder, and one counts what is left in it.
    fn data_dir() -> std::sync::MutexGuard<'static, ()> {
        static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        DIR.get_or_init(|| {
            let d =
                std::env::temp_dir().join(format!("pl-skill-install-{}", harness_core::new_id()));
            std::fs::create_dir_all(&d).unwrap();
            std::env::set_var("PIPELINER_DATA_DIR", &d);
            d
        });
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cfg(dir: &Path) -> Config {
        Config {
            skill_dirs: vec![dir.display().to_string()],
            ..Config::default()
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pl-si-{tag}-{}", harness_core::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn skills_cli_runs_without_prompts() {
        assert_eq!(
            effective_command("npx skills add nextlevelbuilder/ui-ux-pro-max-skill@ui-ux-pro-max"),
            "npx --yes skills add nextlevelbuilder/ui-ux-pro-max-skill@ui-ux-pro-max -a claude-code --copy -y"
        );
        // Flags you gave stay; nothing is doubled.
        assert_eq!(
            effective_command("$ npx -y skills add o/r --agent codex --copy --yes"),
            "npx -y skills add o/r --agent codex --copy --yes"
        );
        assert_eq!(
            effective_command("pnpm dlx skills add o/r --all"),
            "pnpm dlx skills add o/r --all --copy"
        );
        // Other commands and pipelines are left alone.
        assert_eq!(effective_command("npx other-cli add x"), "npx other-cli add x");
        assert_eq!(effective_command("git clone u"), "git clone u");
        assert_eq!(
            effective_command("npx skills add o/r | tee log"),
            "npx skills add o/r | tee log"
        );
    }

    #[test]
    fn plans_github_repo() {
        assert_eq!(
            plan_link("https://github.com/acme/agent-skills").unwrap(),
            LinkPlan::Git {
                clone_url: "https://github.com/acme/agent-skills".into(),
                git_ref: None,
                path: String::new(),
                label: "acme/agent-skills".into(),
            }
        );
        assert_eq!(
            plan_link("github.com/acme/agent-skills.git").unwrap(),
            plan_link("https://github.com/acme/agent-skills/").unwrap()
        );
    }

    #[test]
    fn plans_github_tree_and_blob() {
        assert_eq!(
            plan_link("https://github.com/acme/agent-skills/tree/main/documents").unwrap(),
            LinkPlan::Git {
                clone_url: "https://github.com/acme/agent-skills".into(),
                git_ref: Some("main".into()),
                path: "documents".into(),
                label: "acme/agent-skills/documents".into(),
            }
        );
        assert_eq!(
            plan_link("https://github.com/acme/agent-skills/blob/v2/documents/pdf/SKILL.md")
                .unwrap(),
            LinkPlan::Git {
                clone_url: "https://github.com/acme/agent-skills".into(),
                git_ref: Some("v2".into()),
                path: "documents/pdf".into(),
                label: "acme/agent-skills/documents/pdf".into(),
            }
        );
        // SKILL.md at the repo root.
        match plan_link("https://github.com/a/b/blob/main/SKILL.md").unwrap() {
            LinkPlan::Git { path, .. } => assert_eq!(path, ""),
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn plans_zip_md_other() {
        assert!(matches!(
            plan_link("https://example.com/files/skills.zip?dl=1").unwrap(),
            LinkPlan::Zip { label, .. } if label == "example.com/files/skills.zip"
        ));
        assert!(matches!(
            plan_link("https://raw.githubusercontent.com/a/b/main/x/SKILL.md").unwrap(),
            LinkPlan::Markdown { .. }
        ));
        assert!(matches!(
            plan_link("https://example.com/skill").unwrap(),
            LinkPlan::Other { .. }
        ));
        assert!(plan_link("npx skills add x").is_err());
        assert!(plan_link("https://example.com/a b").is_err());
    }

    #[test]
    fn short_urls() {
        assert_eq!(short_url("https://example.com"), "example.com");
        assert_eq!(
            short_url("https://example.com/a/b/c/d.zip#x"),
            "example.com/…/d.zip"
        );
    }

    #[test]
    fn markdown_goes_to_a_named_folder() {
        let d = tmp("md");
        write_markdown(&d, b"---\nname: Release Notes!\ndescription: x\n---\nbody").unwrap();
        assert!(d.join("Release-Notes").join("SKILL.md").is_file());
        write_markdown(&d, b"---\ndescription: x\n---\n").unwrap();
        assert!(d.join("skill/SKILL.md").is_file());
    }

    #[test]
    fn stages_a_command_run_in_a_temp_home() {
        let _staging = data_dir();
        let mine = tmp("mine");
        let c = cfg(&mine);
        let set = stage_command(
            &c,
            "$ mkdir -p $HOME/.claude/skills/notes/scripts && printf -- '---\\nname: notes\\ndescription: take notes\\n---\\nbody' > $HOME/.claude/skills/notes/SKILL.md && echo 'echo hi' > $HOME/.claude/skills/notes/scripts/run.sh && echo noise > $HOME/other.txt",
            Some(&Origin {
                actor: Some("Orchestrator".into()),
                chat_title: Some("a chat".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        assert_eq!(set.kind, SourceKind::Command);
        assert_eq!(set.skills.len(), 1);
        assert_eq!(set.skills[0].name, "notes");
        assert_eq!(set.skills[0].scripts, 1);
        assert_eq!(set.skills[0].rel_dir, ".claude/skills/notes");
        assert_eq!(set.actor.as_deref(), Some("Orchestrator"));
        // Only skills are kept, the temp home is gone.
        assert_eq!(set.files, 2);
        assert!(!skills::staging_dir(&set.id).join("work").exists());
        // It is listed and can be installed.
        assert!(skills::staged(&c).iter().any(|s| s.id == set.id));
        let out =
            skills::install_staged(&c, &set.id, &[("notes".into(), skills::Clash::Skip)], &mine)
                .unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn failing_command_reports_output_and_leaves_nothing() {
        let _staging = data_dir();
        let c = cfg(&tmp("mine2"));
        let before = skills::staged(&c).len();
        let e = stage_command(&c, "echo boom >&2; exit 3", None).unwrap_err();
        let m = format!("{e:#}");
        assert!(m.contains("exit code 3") && m.contains("boom"), "{m}");
        let e = stage_command(&c, "echo nothing here", None).unwrap_err();
        assert!(format!("{e:#}").contains("found no SKILL.md"), "{e:#}");
        assert!(format!("{e:#}").contains("nothing here"));
        assert_eq!(skills::staged(&c).len(), before);
        let leftovers = std::fs::read_dir(skills::staging_root())
            .map(|r| r.count())
            .unwrap_or(0);
        assert_eq!(leftovers, before);
    }

    #[test]
    fn stages_a_local_git_folder() {
        let _staging = data_dir();
        let repo = tmp("repo");
        let sh = |cmd: &str| {
            let r = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        };
        sh("mkdir -p documents/pdf-tools other/x && printf -- '---\\nname: pdf-tools\\ndescription: pdfs\\n---\\n' > documents/pdf-tools/SKILL.md && printf -- '---\\nname: x\\n---\\n' > other/x/SKILL.md");
        sh("git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -q -m init");
        let c = cfg(&tmp("mine3"));
        let url = format!("file://{}", repo.display());
        let set = stage_git(&c, &url, None, "documents", None).unwrap();
        assert_eq!(set.skills.len(), 1);
        assert_eq!(set.skills[0].name, "pdf-tools");
        assert_eq!(set.skills[0].rel_dir, "pdf-tools");
        assert!(set.commit.as_deref().is_some_and(|c| c.len() >= 7));
        let e = stage_git(&c, &url, None, "nope", None).unwrap_err();
        assert!(format!("{e:#}").contains("no folder `nope`"), "{e:#}");
        skills::discard_staged(&set.id).unwrap();
    }
}
