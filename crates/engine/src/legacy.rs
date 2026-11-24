//! Converting the old app's linear pipelines into the library on import. The old files are
//! only read; each keeps its id (the file stem) so the imported issues that name it resolve.

use std::path::Path;

use anyhow::{Context as _, Result};
use model::{PipelineId, RunnableKind, Severity};
use store::import_v1::{PipelineImport, PipelineOutcome};

use crate::api::{CatalogApi as _, Place};
use crate::engine::Engine;

impl Engine {
    /// Converts every `*.toml` under the old app's pipelines folder and writes it to the user
    /// library, unless a pipeline with its id is there already. A conversion with validation
    /// errors is still written, as a draft that cannot be deployed. `dry_run` writes nothing.
    pub(crate) fn import_v1_pipelines(&self, dry_run: bool) -> Vec<PipelineImport> {
        let paths = &self.inner.paths;
        let mut files: Vec<_> = match std::fs::read_dir(paths.v1_pipelines_dir()) {
            Ok(dir) => dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file()).collect(),
            Err(_) => return Vec::new(),
        };
        files.sort();
        let library = Place::Library.dir(paths, RunnableKind::Pipeline);
        let mut out = Vec::new();
        let mut wrote = false;
        for file in files {
            let name = file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let id = file.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            let outcome = if self.inner.catalog().pipeline(&PipelineId::new(id.as_str())).is_some()
                || library.join(format!("{id}.toml")).exists()
            {
                PipelineOutcome::AlreadyThere
            } else {
                match self.convert_v1_pipeline(&file, &id, dry_run) {
                    Ok(problems) => {
                        wrote |= !dry_run;
                        PipelineOutcome::Converted { problems }
                    }
                    Err(error) => PipelineOutcome::Failed { reason: format!("{error:#}") },
                }
            };
            out.push(PipelineImport { file: name, id, outcome });
        }
        if wrote {
            self.inner.reload_catalog();
        }
        out
    }

    /// Converts one file and (unless `dry_run`) writes it; returns the validator's findings.
    fn convert_v1_pipeline(&self, file: &Path, id: &str, dry_run: bool) -> Result<Vec<String>> {
        let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        let graph = pipeline::legacy::convert_file(Some(id), &text, &self.inner.paths)?;
        let problems = self
            .pipeline_problems(&graph)
            .into_iter()
            .map(|p| {
                let level = if p.severity == Severity::Error { "error" } else { "warning" };
                match &p.node {
                    Some(node) => format!("{level} at {node}: {}", p.message),
                    None => format!("{level}: {}", p.message),
                }
            })
            .collect();
        if !dry_run {
            catalog::write::save_pipeline(&self.inner.paths, &Place::Library, &graph)?;
        }
        Ok(problems)
    }
}
