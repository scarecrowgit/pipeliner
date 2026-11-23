//! Installed skills: folders with a `SKILL.md`, found in the user's, Claude Code's and the
//! project's skill directories.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base::frontmatter;
use base::paths::Paths;
use model::{Skill, SkillSource};

use crate::LoadProblem;

const SKILL_FILE: &str = "SKILL.md";
/// Skill files larger than this are reported and skipped.
const MAX_SKILL_BYTES: u64 = 1024 * 1024;
const MAX_LISTED_FILES: usize = 100;
const MAX_DESCRIPTION_CHARS: usize = 300;

/// Every skill visible from `repo`. A later directory overrides an earlier one with the same
/// name: Claude Code's user skills, then the app's, then the repo's `.claude` and `.harness`.
/// Sorted by name. A skill that cannot be read is reported in `problems`, not returned.
pub(crate) fn discover(paths: &Paths, repo: Option<&Path>, problems: &mut Vec<LoadProblem>) -> Vec<Skill> {
    let mut roots = vec![
        (paths.home.join(".claude/skills"), SkillSource::ClaudeCode),
        (paths.skills_dir(), SkillSource::User),
    ];
    if let Some(repo) = repo {
        roots.push((repo.join(".claude/skills"), SkillSource::Project));
        roots.push((repo.join(".harness/skills"), SkillSource::Project));
    }
    let mut by_name: BTreeMap<String, Skill> = BTreeMap::new();
    for (root, source) in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !is_hidden(p) && p.join(SKILL_FILE).is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            match read_skill(&dir, source) {
                Ok(skill) => {
                    by_name.insert(skill.name.clone(), skill);
                }
                Err(message) => problems.push(LoadProblem { path: dir.join(SKILL_FILE), message }),
            }
        }
    }
    by_name.into_values().collect()
}

fn is_hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn read_skill(dir: &Path, source: SkillSource) -> Result<Skill, String> {
    let file = dir.join(SKILL_FILE);
    let len = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
    if len > MAX_SKILL_BYTES {
        return Err(format!("{SKILL_FILE} is {len} bytes; skills over {MAX_SKILL_BYTES} bytes are skipped"));
    }
    let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let doc = frontmatter::parse(&text);
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = doc.str("name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&folder).to_string();
    let description = doc
        .str("description")
        .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| first_paragraph(doc.body));
    Ok(Skill { name, description, path: dir.to_path_buf(), source, tokens: len / 4 })
}

/// The first paragraph of prose in Markdown, headings skipped, on one line.
fn first_paragraph(body: &str) -> String {
    let mut words: Vec<&str> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            if words.is_empty() {
                continue;
            }
            break;
        }
        if line.is_empty() {
            if words.is_empty() {
                continue;
            }
            break;
        }
        words.extend(line.split_whitespace());
    }
    base::fmt::truncate_chars(&words.join(" "), MAX_DESCRIPTION_CHARS)
}

/// The instructions a model gets when it loads `skill`: the Markdown body without its header,
/// led by the skill's folder so relative references in it can be resolved, and followed by the
/// other files in that folder. `None` when the file can no longer be read.
pub(crate) fn skill_text(skill: &Skill) -> Option<String> {
    let text = std::fs::read_to_string(skill.path.join(SKILL_FILE)).ok()?;
    let body = frontmatter::parse(&text).body.trim().to_string();
    let mut out = format!("Base directory for this skill: {}\n\n{body}\n", skill.path.display());
    let mut files = Vec::new();
    list_files(&skill.path, &skill.path, 0, &mut files);
    files.sort();
    if !files.is_empty() {
        out.push_str("\nOther files in this skill folder (relative to the base directory):\n");
        for f in files.iter().take(MAX_LISTED_FILES) {
            out.push_str(&format!("- {f}\n"));
        }
        if files.len() > MAX_LISTED_FILES {
            out.push_str(&format!("- … {} more\n", files.len() - MAX_LISTED_FILES));
        }
    }
    Some(out)
}

fn list_files(base: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 4 || out.len() > MAX_LISTED_FILES * 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || matches!(name.as_str(), "node_modules" | "target" | "__pycache__") {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            list_files(base, &path, depth + 1, out);
        } else if kind.is_file()
            && !(depth == 0 && name == SKILL_FILE)
            && let Ok(rel) = path.strip_prefix(base)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn skill(root: &Path, folder: &str, text: &str) -> PathBuf {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SKILL_FILE), text).unwrap();
        dir
    }

    #[test]
    fn name_and_description_come_from_the_header_or_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        let root = paths.skills_dir();
        skill(&root, "pdf-tools", "---\nname: pdf\ndescription: >\n  Read and\n  merge PDFs\n---\n# PDF\nBody");
        skill(&root, "plain", "# Plain skill\n\nDoes the plain thing\nover two lines.\n\nSecond paragraph.");
        skill(&root, "headerless", "---\nversion: 2\n---\nWrite tidy commit messages.");
        fs::create_dir_all(root.join("not-a-skill")).unwrap();
        let mut problems = Vec::new();
        let skills = discover(&paths, None, &mut problems);
        assert!(problems.is_empty());
        let names: Vec<_> = skills.iter().map(|s| (s.name.as_str(), s.description.as_str())).collect();
        assert_eq!(
            names,
            [
                ("headerless", "Write tidy commit messages."),
                ("pdf", "Read and merge PDFs"),
                ("plain", "Does the plain thing over two lines."),
            ]
        );
        assert!(skills.iter().all(|s| s.source == SkillSource::User));
        let pdf = skills.iter().find(|s| s.name == "pdf").unwrap();
        assert_eq!(pdf.tokens, std::fs::metadata(pdf.path.join(SKILL_FILE)).unwrap().len() / 4);
    }

    #[test]
    fn later_roots_override_earlier_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        let repo = tmp.path().join("repo");
        skill(&paths.home.join(".claude/skills"), "a", "---\ndescription: claude home\n---\n");
        skill(&paths.skills_dir(), "a", "---\ndescription: user\n---\n");
        skill(&paths.skills_dir(), "only-user", "---\ndescription: u\n---\n");
        skill(&repo.join(".claude/skills"), "a", "---\ndescription: repo claude\n---\n");
        skill(&repo.join(".harness/skills"), "b", "---\ndescription: repo harness b\n---\n");
        skill(&repo.join(".claude/skills"), "b", "---\ndescription: repo claude b\n---\n");
        let skills = discover(&paths, Some(&repo), &mut Vec::new());
        let get = |n: &str| skills.iter().find(|s| s.name == n).unwrap();
        assert_eq!(get("a").description, "repo claude");
        assert_eq!(get("a").source, SkillSource::Project);
        assert_eq!(get("b").description, "repo harness b");
        assert_eq!(get("only-user").source, SkillSource::User);
        let without_repo = discover(&paths, None, &mut Vec::new());
        assert_eq!(without_repo.iter().find(|s| s.name == "a").unwrap().description, "user");
    }

    #[test]
    fn text_has_the_folder_and_no_header() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = skill(tmp.path(), "s", "---\nname: s\ndescription: d\n---\n# Use it\nRun `scripts/go.sh`.\n");
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("scripts/go.sh"), "echo").unwrap();
        let found = read_skill(&dir, SkillSource::User).unwrap();
        let text = skill_text(&found).unwrap();
        assert!(text.starts_with(&format!("Base directory for this skill: {}", dir.display())), "{text}");
        assert!(text.contains("# Use it") && !text.contains("description: d"), "{text}");
        assert!(text.contains("- scripts/go.sh") && !text.contains("- SKILL.md"), "{text}");
    }
}
