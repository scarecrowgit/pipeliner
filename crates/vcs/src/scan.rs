//! Looking at folders: what an agent would find in one, and which repositories are below one.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use base::cancel::CancelToken;

use crate::facts::{RepoFacts, facts};

/// Folders that are never searched for repositories.
const SKIPPED: [&str; 2] = ["target", "node_modules"];

/// What the new-chat page shows under "Found in this folder".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderInfo {
    /// Lines of `AGENTS.md`, or of `CLAUDE.md` when there is no `AGENTS.md`.
    pub agents_md_lines: Option<u32>,
    /// Skills under `.harness/skills` and `.claude/skills`: folders that hold a `SKILL.md`.
    pub skills: Vec<String>,
    /// Commands under `.harness/commands`: the names of the `.md` files.
    pub commands: Vec<String>,
    /// Git facts when the folder is inside a repository.
    pub facts: Option<RepoFacts>,
}

/// Looks into `path` for instructions, skills and commands, and asks git about it.
pub fn folder(path: &Path, cancel: &CancelToken) -> FolderInfo {
    let agents_md_lines = ["AGENTS.md", "CLAUDE.md"].iter().find_map(|name| {
        let bytes = std::fs::read(path.join(name)).ok()?;
        let lines = String::from_utf8_lossy(&bytes).lines().count();
        Some(u32::try_from(lines).unwrap_or(u32::MAX))
    });

    let mut skills = BTreeSet::new();
    for root in [".harness/skills", ".claude/skills"] {
        let Ok(entries) = std::fs::read_dir(path.join(root)) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            if dir.is_dir() && dir.join("SKILL.md").is_file() {
                skills.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }

    let mut commands = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(path.join(".harness/commands")) {
        for entry in entries.flatten() {
            let file = entry.path();
            if file.is_file()
                && file.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
                && let Some(stem) = file.file_stem()
            {
                commands.insert(stem.to_string_lossy().into_owned());
            }
        }
    }

    FolderInfo {
        agents_md_lines,
        skills: skills.into_iter().collect(),
        commands: commands.into_iter().collect(),
        facts: facts(path, cancel),
    }
}

/// Git checkouts below `root`, at most `limit`, shallow ones first and sorted by path within a
/// level. Looks `max_depth` levels down (1 is the direct children). A checkout (a folder with
/// `.git`, a linked worktree included) is not searched further, and hidden folders, `target`
/// and `node_modules` are skipped. `root` itself is not reported.
pub fn repos_under(root: &Path, max_depth: u32, limit: usize, cancel: &CancelToken) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), 0u32)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                !name.starts_with('.') && !SKIPPED.contains(&name.as_ref())
            })
            .map(|entry| entry.path())
            .collect();
        children.sort();
        for child in children {
            if cancel.is_cancelled() {
                return found;
            }
            if child.join(".git").exists() {
                found.push(child);
                if found.len() >= limit {
                    return found;
                }
            } else {
                queue.push_back((child, depth + 1));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, git, write};

    fn cancel() -> CancelToken {
        CancelToken::new()
    }

    #[test]
    fn folder_reports_instructions_skills_and_commands() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("scan");
        write(&repo, "AGENTS.md", "one\ntwo\nthree\n");
        write(&repo, "CLAUDE.md", "ignored when AGENTS.md exists\n");
        write(&repo, ".harness/skills/deploy/SKILL.md", "---\nname: deploy\n---\n");
        write(&repo, ".claude/skills/review/SKILL.md", "x");
        write(&repo, ".claude/skills/not-a-skill/readme.txt", "x");
        write(&repo, ".harness/commands/ship.md", "x");
        write(&repo, ".harness/commands/fix.MD", "x");
        write(&repo, ".harness/commands/notes.txt", "x");
        let info = folder(&repo, &cancel());
        assert_eq!(info.agents_md_lines, Some(3));
        assert_eq!(info.skills, ["deploy", "review"]);
        assert_eq!(info.commands, ["fix", "ship"]);
        assert_eq!(info.facts.expect("facts").name, "scan");
    }

    #[test]
    fn folder_falls_back_to_claude_md_and_handles_plain_folders() {
        let sandbox = Sandbox::new();
        let plain = sandbox.path().join("plain");
        write(&plain, "CLAUDE.md", "a\nb");
        let info = folder(&plain, &cancel());
        assert_eq!(info.agents_md_lines, Some(2));
        assert!(info.skills.is_empty() && info.commands.is_empty());
        assert_eq!(info.facts, None);
        assert_eq!(folder(&sandbox.path().join("missing"), &cancel()), FolderInfo::default());
    }

    #[test]
    fn repos_are_found_below_a_folder_without_descending_into_them() {
        let sandbox = Sandbox::new();
        let root = sandbox.path().join("code");
        std::fs::create_dir_all(&root).unwrap();
        for name in ["b-repo", "a-repo"] {
            let repo = root.join(name);
            std::fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init", "-q"]);
            // A repo inside a repo is not reported.
            std::fs::create_dir_all(repo.join("vendor/inner/.git")).unwrap();
        }
        let nested = root.join("group/deep");
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        for skipped in ["node_modules/pkg", "target/x", ".hidden/y"] {
            let dir = root.join(skipped);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q"]);
        }
        // A linked worktree has a `.git` file and counts as a checkout.
        let main = sandbox.repo("elsewhere");
        let wt = root.join("wt");
        git(&main, &["worktree", "add", "-q", "-b", "w", wt.to_str().unwrap()]);

        let all = repos_under(&root, 3, 50, &cancel());
        assert_eq!(all, [root.join("a-repo"), root.join("b-repo"), root.join("wt"), root.join("group/deep")]);
        // Depth 1 sees direct children only.
        assert_eq!(repos_under(&root, 1, 50, &cancel()).len(), 3);
        assert_eq!(repos_under(&root, 0, 50, &cancel()), Vec::<PathBuf>::new());
        // The limit stops the search.
        assert_eq!(repos_under(&root, 3, 2, &cancel()), [root.join("a-repo"), root.join("b-repo")]);
        assert!(repos_under(&sandbox.path().join("missing"), 3, 5, &cancel()).is_empty());
    }
}
