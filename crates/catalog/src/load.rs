//! Reading every location into a [`Catalog`].

use std::path::{Path, PathBuf};

use base::paths::Paths;
use model::{GraphDef, Origin, PipelineId, Runnable, RunnableKind, RunnableRef, Uses};
use serde::Deserialize;

use crate::agent_file::parse_agent;
use crate::place::Place;
use crate::{Catalog, LoadProblem, PipelineEntry, builtin, skills};

/// Files larger than this are reported and skipped.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The top-level switches of a pipeline file that are not part of the graph itself.
#[derive(Default, Deserialize)]
pub(crate) struct PipelineFlags {
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub archived: bool,
}

impl Catalog {
    /// Reads every location: built-ins, Claude Code agents (`~/.claude/agents`, then the repo's),
    /// the user library and the project's `.harness`. A later one overrides an earlier one with
    /// the same id, so a file you wrote in the library or project is never hidden by an import.
    /// Never fails as a whole: a file that does not parse becomes a [`LoadProblem`].
    pub fn load(paths: &Paths, repo: Option<&Path>) -> Catalog {
        let mut cat = Catalog::default();
        for runnable in builtin::builtin_runnables() {
            cat.add(runnable);
        }
        let mut claude_dirs = vec![paths.home.join(".claude/agents")];
        claude_dirs.extend(repo.map(|r| r.join(".claude/agents")));
        for dir in claude_dirs {
            for file in files_in(&dir, "md") {
                cat.read_claude_agent(&file);
            }
        }
        let mut places = vec![Place::Library];
        places.extend(repo.map(|r| Place::Project(r.to_path_buf())));
        for place in places {
            for kind in [RunnableKind::Agent, RunnableKind::Orchestrator] {
                for file in files_in(&place.dir(paths, kind), "md") {
                    cat.read_agent(&file, kind, &place);
                }
            }
            for file in files_in(&place.dir(paths, RunnableKind::Pipeline), "toml") {
                cat.read_pipeline(&file, &place);
            }
        }
        cat.skills = skills::discover(paths, repo, &mut cat.problems);
        cat.finish();
        cat
    }

    fn add(&mut self, runnable: Runnable) {
        self.by_ref.insert(runnable.reference.clone(), runnable);
    }

    fn problem(&mut self, path: &Path, message: impl ToString) {
        self.problems.push(LoadProblem { path: path.to_path_buf(), message: message.to_string() });
    }

    fn read_claude_agent(&mut self, file: &Path) {
        if file.file_name().is_some_and(|n| n.eq_ignore_ascii_case("README.md")) {
            return;
        }
        let Some((id, text)) = self.read(file) else { return };
        match parse_agent(&text, &id, true) {
            Ok(mut parsed) => {
                // Claude Code has agents only; whatever the header says, this is one.
                parsed.kind = RunnableKind::Agent;
                parsed.uses = Uses::default();
                let origin = Origin::ClaudeCode { path: file.to_path_buf() };
                self.add(parsed.into_runnable(&id, origin, Some(file.to_path_buf())));
            }
            Err(e) => self.problem(file, e),
        }
    }

    fn read_agent(&mut self, file: &Path, kind: RunnableKind, place: &Place) {
        let Some((id, text)) = self.read(file) else { return };
        match parse_agent(&text, &id, false) {
            Ok(mut parsed) => {
                // The folder decides what the file is.
                parsed.kind = kind;
                self.add(parsed.into_runnable(&id, place.origin(), Some(file.to_path_buf())));
            }
            Err(e) => self.problem(file, e),
        }
    }

    fn read_pipeline(&mut self, file: &Path, place: &Place) {
        let Some((id, text)) = self.read(file) else { return };
        let parsed = toml::from_str::<GraphDef>(&text).map_err(|e| e.to_string());
        let flags = toml::from_str::<PipelineFlags>(&text).unwrap_or_default();
        match parsed {
            Ok(mut graph) => {
                graph.id = PipelineId::new(&id);
                self.add(Runnable {
                    reference: RunnableRef::pipeline(&id),
                    kind: RunnableKind::Pipeline,
                    name: graph.name.clone(),
                    description: graph.description.clone(),
                    origin: place.origin(),
                    uses: Uses::default(),
                    paused: flags.paused,
                    archived: flags.archived,
                    agent: None,
                    path: Some(file.to_path_buf()),
                });
                self.pipelines.insert(PipelineId::new(&id), PipelineEntry { graph, text });
            }
            Err(e) => self.problem(file, e),
        }
    }

    /// The text of `file` and its stem, or a problem when it cannot be read.
    fn read(&mut self, file: &Path) -> Option<(String, String)> {
        let id = file.file_stem()?.to_string_lossy().into_owned();
        let len = std::fs::metadata(file).map(|m| m.len());
        match len {
            Ok(len) if len > MAX_FILE_BYTES => {
                self.problem(file, format!("{len} bytes; files over {MAX_FILE_BYTES} bytes are skipped"));
                return None;
            }
            Err(e) => {
                self.problem(file, e);
                return None;
            }
            Ok(_) => {}
        }
        match std::fs::read_to_string(file) {
            Ok(text) => Some((id, text)),
            Err(e) => {
                self.problem(file, e);
                None
            }
        }
    }

    /// Sorts the list: orchestrators, pipelines, agents, each by name.
    fn finish(&mut self) {
        let rank = |k: RunnableKind| match k {
            RunnableKind::Orchestrator => 0,
            RunnableKind::Pipeline => 1,
            RunnableKind::Agent => 2,
        };
        let mut all: Vec<Runnable> = std::mem::take(&mut self.by_ref).into_values().collect();
        all.sort_by(|a, b| {
            (rank(a.kind), a.name.to_lowercase(), &a.reference).cmp(&(rank(b.kind), b.name.to_lowercase(), &b.reference))
        });
        self.index = all.iter().enumerate().map(|(i, r)| (r.reference.clone(), i)).collect();
        self.runnables = all;
    }
}

/// Files with extension `ext` directly in `dir`, sorted; none when the folder is missing.
fn files_in(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    files.sort();
    files
}
