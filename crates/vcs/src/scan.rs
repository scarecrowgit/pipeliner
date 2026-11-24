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

/// A folder inside another, as path completion offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildDir {
    /// The folder's own name.
    pub name: String,
    /// `dir` joined with `name`.
    pub path: PathBuf,
    /// It is a git checkout (it has a `.git` folder or file).
    pub is_repo: bool,
}

/// The folders directly inside `dir` whose name starts with `prefix` (ignoring case), at most
/// `limit`: visible ones by name, then hidden ones. Symlinks to folders count; folders that
/// cannot be read are left out. Fails when `dir` itself cannot be read.
pub fn child_dirs(dir: &Path, prefix: &str, limit: usize) -> std::io::Result<Vec<ChildDir>> {
    let wanted = prefix.to_lowercase();
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.to_lowercase().starts_with(&wanted))
        .collect();
    names.sort_by_key(|name| (name.starts_with('.'), name.to_lowercase(), name.clone()));
    let mut out = Vec::new();
    for name in names {
        if out.len() >= limit {
            break;
        }
        let path = dir.join(&name);
        // `metadata` follows a symlink; reading the folder proves it can be entered.
        if !std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) || std::fs::read_dir(&path).is_err() {
            continue;
        }
        let is_repo = path.join(".git").exists();
        out.push(ChildDir { name, path, is_repo });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, git, write};

    #[test]
    fn child_dirs_list_matching_folders_visible_first() {
        let sandbox = Sandbox::new();
        let root = sandbox.path().join("code");
        for dir in ["beta", "Alpha", "alps", ".hidden-a", "other"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        write(&root, "a-file.txt", "not a folder");
        git(&root.join("alps"), &["init", "-q"]);
        let names = |prefix: &str, limit: usize| -> Vec<String> {
            child_dirs(&root, prefix, limit).unwrap().into_iter().map(|c| c.name).collect()
        };
        assert_eq!(names("", 10), ["Alpha", "alps", "beta", "other", ".hidden-a"]);
        assert_eq!(names("al", 10), ["Alpha", "alps"], "case is ignored and files are left out");
        assert_eq!(names("", 2), ["Alpha", "alps"]);
        assert_eq!(names(".h", 10), [".hidden-a"]);
        let alps = child_dirs(&root, "alps", 1).unwrap().remove(0);
        assert_eq!((alps.path, alps.is_repo), (root.join("alps"), true));
        assert!(!child_dirs(&root, "beta", 1).unwrap()[0].is_repo);
        assert!(child_dirs(&root.join("missing"), "", 5).is_err());
        assert!(child_dirs(&root.join("a-file.txt"), "", 5).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn child_dirs_follow_symlinks_but_skip_unreadable_folders() {
        use std::os::unix::fs::PermissionsExt as _;
        let sandbox = Sandbox::new();
        let root = sandbox.path().join("root");
        std::fs::create_dir_all(root.join("open")).unwrap();
        std::fs::create_dir_all(root.join("locked")).unwrap();
        std::fs::create_dir_all(sandbox.path().join("target-dir")).unwrap();
        std::os::unix::fs::symlink(sandbox.path().join("target-dir"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(sandbox.path().join("gone"), root.join("dangling")).unwrap();
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read anything; the check only means something for an ordinary user.
        let unreadable = std::fs::read_dir(root.join("locked")).is_err();
        let names: Vec<String> = child_dirs(&root, "", 10).unwrap().into_iter().map(|c| c.name).collect();
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        if unreadable {
            assert_eq!(names, ["link", "open"]);
        } else {
            assert_eq!(names, ["link", "locked", "open"]);
        }
    }

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
