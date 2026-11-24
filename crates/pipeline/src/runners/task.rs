//! What an agent node is told: the item, what happened before it, the files attached to it,
//! the files it must leave, and the review's notes.

use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use model::{FileMode, NodeKind};

use super::Ctx;
use super::places::Place;

/// Most context files listed by name.
const CONTEXT_LIST: usize = 120;
/// Most entries looked at while matching a context glob.
const WALK_LIMIT: usize = 20_000;

/// A file the agent must write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Required {
    /// As shown to the agent.
    pub shown: String,
    pub path: PathBuf,
}

/// The task text and the files the agent must leave behind.
pub(crate) struct Task {
    pub text: String,
    pub required: Vec<Required>,
}

impl Ctx<'_> {
    /// `{key}`, `{run}`, `{variant}` and `{node}` in a file node's path template.
    pub fn expand(&self, template: &str) -> String {
        let job = self.job;
        let key = job.item.issue.as_ref().map_or_else(|| job.run.to_string(), |issue| issue.key.to_string());
        template
            .replace("{key}", &key)
            .replace("{run}", job.run.as_str())
            .replace("{variant}", &model::variant_letter(job.key.variant))
            .replace("{node}", job.key.node.as_str())
    }

    /// Builds the task of an agent working in `place` (in `workdir`), with `files_out` relative to it.
    pub fn task(&self, place: Option<&Place>, workdir: &str, files_out: &[String]) -> Task {
        let job = self.job;
        let mut text = format!("# {}\n\n", self.title());
        if !job.item.text.trim().is_empty() {
            text.push_str(job.item.text.trim());
            text.push_str("\n\n");
        }
        if let Some(issue) = &job.item.issue {
            text.push_str(&format!(
                "Issue {} · {} · {} priority · {}\n\n",
                issue.key,
                issue.kind.label(),
                issue.priority.label(),
                issue.status.label()
            ));
        }
        let earlier: Vec<String> = job
            .upstream
            .iter()
            .filter(|up| !up.output.summary.trim().is_empty())
            .map(|up| format!("- **{}**: {}", up.name, up.output.summary.trim()))
            .collect();
        if !earlier.is_empty() {
            text.push_str("## Earlier steps\n\n");
            text.push_str(&earlier.join("\n"));
            text.push_str("\n\n");
        }
        if let Some(place) = place {
            match &place.branch {
                Some(branch) => text.push_str(&format!(
                    "You work in {} on branch `{branch}` (base `{}`). Leave your changes uncommitted unless your role says otherwise.\n\n",
                    place.path.display(),
                    place.base
                )),
                None => text.push_str(&format!("You work in {}.\n\n", place.path.display())),
            }
        }
        let mut required: Vec<Required> = Vec::new();
        for path in files_out {
            if let Some(place) = place {
                let shown = self.expand(path);
                required.push(Required { path: place.path.join(&shown), shown });
            }
        }
        for node in &job.attached {
            let NodeKind::File(file) = &node.kind else { continue };
            let wd = if file.workdir.is_empty() { workdir } else { file.workdir.as_str() };
            let root = match (self.workdir(wd), place) {
                (Ok(def), Some(place)) if def.id == workdir => place.path.clone(),
                (Ok(def), _) => self.read_place(def),
                (Err(_), Some(place)) => place.path.clone(),
                (Err(_), None) => continue,
            };
            match file.mode {
                FileMode::Read => text.push_str(&context_section(&node.name, &root, &self.expand(&file.path))),
                FileMode::Write => {
                    let shown = self.expand(&file.path);
                    required.push(Required { path: root.join(&shown), shown });
                }
            }
        }
        if !required.is_empty() {
            text.push_str("## Files you must write\n\n");
            for file in &required {
                text.push_str(&format!("- `{}` ({})\n", file.shown, file.path.display()));
            }
            text.push('\n');
        }
        if let Some(retry) = &job.retry
            && !retry.notes.is_empty()
        {
            text.push_str(&feedback(&retry.notes));
        }
        text.push_str("When the work is complete, call `step_done` with a short summary: what you did and how you checked it.\n");
        Task { text, required }
    }
}

/// The review notes as the agent reads them.
pub(crate) fn feedback(notes: &[String]) -> String {
    let mut text = String::from("## Review feedback\n\nA person reviewed earlier work and sent it back:\n\n");
    for (i, note) in notes.iter().enumerate() {
        text.push_str(&format!("{}. {}\n", i + 1, note.trim()));
    }
    text.push_str("\nAddress the latest note first.\n\n");
    text
}

/// "## Context: Mockups" with the files of `root` matching `glob`.
fn context_section(name: &str, root: &Path, glob: &str) -> String {
    let files = matching(root, glob);
    let mut text = format!("## Context: {name}\n\nRead these files as you need them (`{glob}` in {}):\n\n", root.display());
    if files.is_empty() {
        text.push_str("- (no files match yet)\n");
    }
    for file in files.iter().take(CONTEXT_LIST) {
        text.push_str(&format!("- {file}\n"));
    }
    if files.len() > CONTEXT_LIST {
        text.push_str(&format!("- … and {} more\n", files.len() - CONTEXT_LIST));
    }
    text.push('\n');
    text
}

/// Files below `root` whose relative path matches `glob`, sorted; `.git` is skipped.
pub(crate) fn matching(root: &Path, glob: &str) -> Vec<String> {
    let Ok(glob) = GlobBuilder::new(glob.trim()).literal_separator(true).build() else { return Vec::new() };
    let matcher = glob.compile_matcher();
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > WALK_LIMIT {
                break;
            }
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(_) => {
                    if let Ok(rel) = path.strip_prefix(root)
                        && matcher.is_match(rel)
                    {
                        out.push(rel.to_string_lossy().into_owned());
                    }
                }
                Err(_) => {}
            }
        }
    }
    out.sort();
    out
}

/// Whether `path` (relative) matches `glob`.
pub(crate) fn glob_matches(glob: &str, path: &str) -> bool {
    GlobBuilder::new(glob.trim())
        .literal_separator(true)
        .build()
        .is_ok_and(|g| g.compile_matcher().is_match(path))
}
