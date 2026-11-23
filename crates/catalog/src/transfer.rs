//! Moving definitions in and out of the catalog as single files.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use base::frontmatter;
use base::paths::Paths;
use model::{GraphDef, PipelineId, RunnableKind, RunnableRef};

use crate::agent_file::parse_agent;
use crate::place::{Place, file_in};
use crate::write::{free_id, reference_of, render_pipeline, slug, write_atomic};
use crate::{Catalog, load::PipelineFlags};

/// Writes the definition to `dest`: an agent or orchestrator as its Markdown file, a pipeline as
/// its TOML file. Works for built-ins and imports too.
pub fn export(catalog: &Catalog, reference: &RunnableRef, dest: &Path) -> Result<()> {
    let runnable = catalog.get(reference).ok_or_else(|| anyhow!("`{reference}` does not exist"))?;
    let text = if runnable.kind == RunnableKind::Pipeline {
        let id = PipelineId::new(reference.id());
        match catalog.pipeline_text(&id) {
            Some(text) => text.to_string(),
            None => bail!("`{reference}` has no definition"),
        }
    } else {
        crate::agent_file::render_agent(runnable)?
    };
    write_atomic(dest, &text)
}

/// Reads an agent file (`.md` with a header) or a pipeline file (TOML) from `src` into `place`
/// under a free id and returns its reference. Which of the two it is comes from the content, not
/// from the extension. A Claude Code agent file works as is.
pub fn import(catalog: &Catalog, paths: &Paths, place: &Place, src: &Path) -> Result<RunnableRef> {
    let text = std::fs::read_to_string(src).with_context(|| format!("cannot read {}", src.display()))?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.trim_start().starts_with("---") {
        let doc = frontmatter::parse(text);
        let claude_code = doc.map("harness").is_none();
        let parsed = parse_agent(text, &stem, claude_code).with_context(|| format!("{} is not a valid agent file", src.display()))?;
        let dir = place.dir(paths, parsed.kind);
        let id = free_id(catalog, parsed.kind, &dir, &slug(&stem));
        let runnable = parsed.into_runnable(&id, place.origin(), None);
        crate::write::save_agent(paths, place, &runnable)?;
        return Ok(runnable.reference);
    }
    let mut graph: GraphDef = toml::from_str(text).map_err(|e| {
        anyhow!("{} is neither an agent file (a `---` header) nor a pipeline (TOML): {e}", src.display())
    })?;
    let wanted = if graph.id.as_str().is_empty() { stem } else { graph.id.to_string() };
    let dir = place.dir(paths, RunnableKind::Pipeline);
    let id = free_id(catalog, RunnableKind::Pipeline, &dir, &slug(&wanted));
    graph.id = PipelineId::new(&id);
    let flags = PipelineFlags::default();
    write_atomic(&file_in(&dir, RunnableKind::Pipeline, &id), &render_pipeline(&graph, &flags)?)?;
    Ok(reference_of(RunnableKind::Pipeline, &id))
}
