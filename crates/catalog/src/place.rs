//! Where a definition is written: the user library or a project.

use std::path::{Path, PathBuf};

use base::paths::Paths;
use model::{Origin, RunnableKind};

/// A place definitions live in and can be written to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// `<config>/library/`, for every project.
    Library,
    /// `<repo>/.harness/`, checked in with the project.
    Project(PathBuf),
}

impl Place {
    /// The folder holding files of `kind`: `agents`, `orchestrators` or `pipelines`.
    pub fn dir(&self, paths: &Paths, kind: RunnableKind) -> PathBuf {
        let sub = kind_dir(kind);
        match self {
            Place::Library => paths.library_dir().join(sub),
            Place::Project(repo) => repo.join(".harness").join(sub),
        }
    }

    /// The origin runnables read from this place report.
    pub fn origin(&self) -> Origin {
        match self {
            Place::Library => Origin::Library,
            Place::Project(repo) => Origin::Project { repo: repo.clone() },
        }
    }

    /// The place a runnable with `origin` was loaded from; `None` for built-ins, Claude Code
    /// imports and created agents, which have no file of their own to edit.
    pub fn of(origin: &Origin) -> Option<Place> {
        match origin {
            Origin::Library => Some(Place::Library),
            Origin::Project { repo } => Some(Place::Project(repo.clone())),
            _ => None,
        }
    }
}

pub(crate) fn kind_dir(kind: RunnableKind) -> &'static str {
    match kind {
        RunnableKind::Agent => "agents",
        RunnableKind::Orchestrator => "orchestrators",
        RunnableKind::Pipeline => "pipelines",
    }
}

/// The file of `id` in `dir`: Markdown for agents and orchestrators, TOML for pipelines.
pub(crate) fn file_in(dir: &Path, kind: RunnableKind, id: &str) -> PathBuf {
    let ext = if kind == RunnableKind::Pipeline { "toml" } else { "md" };
    dir.join(format!("{id}.{ext}"))
}
