//! Changing the files of the catalog. Every function writes and returns; reload the
//! [`Catalog`] afterwards to see the result.
//!
//! Built-ins and Claude Code imports are never written: the functions that would change them
//! fail with a message that says to duplicate them first.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use base::paths::Paths;
use model::{GraphDef, Origin, Runnable, RunnableKind, RunnableRef};

use crate::agent_file::render_agent;
use crate::load::PipelineFlags;
use crate::place::{Place, file_in};
use crate::Catalog;

/// Longest id a name is turned into.
const MAX_ID_LEN: usize = 48;

/// Writes an agent or orchestrator to `place`, replacing the file of the same id. Fails for
/// built-ins and Claude Code imports.
pub fn save_agent(paths: &Paths, place: &Place, runnable: &Runnable) -> Result<PathBuf> {
    if runnable.kind == RunnableKind::Pipeline {
        bail!("`{}` is a pipeline; save it with save_pipeline", runnable.reference);
    }
    check_writable(runnable)?;
    check_id(runnable.reference.id())?;
    let path = file_in(&place.dir(paths, runnable.kind), runnable.kind, runnable.reference.id());
    write_atomic(&path, &render_agent(runnable)?)?;
    Ok(path)
}

/// Writes a pipeline's graph to `place` through a temp file and a rename. The pipeline's
/// paused and archived switches, kept at the top of the file, survive.
pub fn save_pipeline(paths: &Paths, place: &Place, graph: &GraphDef) -> Result<PathBuf> {
    check_id(graph.id.as_str())?;
    let path = file_in(&place.dir(paths, RunnableKind::Pipeline), RunnableKind::Pipeline, graph.id.as_str());
    let flags = read_flags(&path);
    write_atomic(&path, &render_pipeline(graph, &flags)?)?;
    Ok(path)
}

/// Removes the file of a library or project definition.
pub fn delete(catalog: &Catalog, reference: &RunnableRef) -> Result<()> {
    let runnable = editable(catalog, reference)?;
    let path = file_of(runnable)?;
    std::fs::remove_file(path).with_context(|| format!("cannot delete {}", path.display()))
}

/// A copy named "<name> copy" (then "copy 2"…) with a new unique id, written to `place`. Works on
/// anything with a definition, built-ins and imports included. Returns the copy's reference.
pub fn duplicate(catalog: &Catalog, paths: &Paths, place: &Place, reference: &RunnableRef) -> Result<RunnableRef> {
    let source = catalog.get(reference).ok_or_else(|| anyhow!("`{reference}` does not exist"))?;
    let dir = place.dir(paths, source.kind);
    let mut name = format!("{} copy", source.name);
    let mut n = 2;
    while id_taken(catalog, source.kind, &dir, &slug(&name)) {
        name = format!("{} copy {n}", source.name);
        n += 1;
    }
    let id = slug(&name);
    let copy_ref = reference_of(source.kind, &id);
    if source.kind == RunnableKind::Pipeline {
        let mut graph = catalog.pipeline(&model::PipelineId::new(reference.id())).cloned().ok_or_else(|| anyhow!("`{reference}` has no definition"))?;
        graph.id = model::PipelineId::new(&id);
        graph.name = name;
        save_pipeline(paths, place, &graph)?;
    } else {
        let mut copy = source.clone();
        copy.reference = copy_ref.clone();
        copy.name = name;
        copy.origin = place.origin();
        copy.path = None;
        copy.paused = false;
        copy.archived = false;
        save_agent(paths, place, &copy)?;
    }
    Ok(copy_ref)
}

/// Gives a library or project definition a new name; its id (and file) follow through
/// `base::id::slug`. Fails without changing anything when another definition of the same kind
/// already has that id. References to the old id elsewhere (a team list, a pipeline node)
/// are not rewritten. Returns the new reference.
pub fn rename(catalog: &Catalog, reference: &RunnableRef, new_name: &str) -> Result<RunnableRef> {
    let runnable = editable(catalog, reference)?;
    let old_path = file_of(runnable)?;
    let name = new_name.trim();
    if name.is_empty() {
        bail!("a name cannot be empty");
    }
    let id = slug(name);
    let new_ref = reference_of(runnable.kind, &id);
    let dir = old_path.parent().ok_or_else(|| anyhow!("{} has no folder", old_path.display()))?;
    let new_path = file_in(dir, runnable.kind, &id);
    if new_ref != *reference && (catalog.get(&new_ref).is_some() || new_path.exists()) {
        bail!("`{id}` is already taken by another {}; pick a different name", kind_word(runnable.kind));
    }
    if runnable.kind == RunnableKind::Pipeline {
        let mut graph = catalog.pipeline(&model::PipelineId::new(reference.id())).cloned().ok_or_else(|| anyhow!("`{reference}` has no definition"))?;
        graph.id = model::PipelineId::new(&id);
        graph.name = name.to_string();
        let flags = PipelineFlags { paused: runnable.paused, archived: runnable.archived };
        write_atomic(&new_path, &render_pipeline(&graph, &flags)?)?;
    } else {
        let mut renamed = runnable.clone();
        renamed.reference = new_ref.clone();
        renamed.name = name.to_string();
        write_atomic(&new_path, &render_agent(&renamed)?)?;
    }
    if new_path != *old_path {
        std::fs::remove_file(old_path).with_context(|| format!("cannot remove {}", old_path.display()))?;
    }
    Ok(new_ref)
}

/// Pauses or resumes: a paused definition finishes what it runs and takes no new work.
pub fn set_paused(catalog: &Catalog, reference: &RunnableRef, paused: bool) -> Result<()> {
    set_flag(catalog, reference, "paused", paused)
}

/// Archives or restores: an archived definition is hidden from pickers and the default list.
pub fn set_archived(catalog: &Catalog, reference: &RunnableRef, archived: bool) -> Result<()> {
    set_flag(catalog, reference, "archived", archived)
}

fn set_flag(catalog: &Catalog, reference: &RunnableRef, key: &str, on: bool) -> Result<()> {
    let runnable = editable(catalog, reference)?;
    let path = file_of(runnable)?;
    if runnable.kind == RunnableKind::Pipeline {
        let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        return write_atomic(path, &with_flag(&text, key, on));
    }
    let mut changed = runnable.clone();
    match key {
        "paused" => changed.paused = on,
        _ => changed.archived = on,
    }
    write_atomic(path, &render_agent(&changed)?)
}

/// `text` with the top-level `key = true` line added or removed. The switches of a pipeline
/// file sit before its first table, where the graph itself does not write anything.
fn with_flag(text: &str, key: &str, on: bool) -> String {
    let mut out = String::new();
    if on {
        out.push_str(&format!("{key} = true\n"));
    }
    let mut in_table = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        in_table |= trimmed.starts_with('[');
        let is_flag = !in_table
            && trimmed.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='));
        if !is_flag {
            out.push_str(line);
        }
    }
    out
}

fn read_flags(path: &Path) -> PipelineFlags {
    std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

pub(crate) fn render_pipeline(graph: &GraphDef, flags: &PipelineFlags) -> Result<String> {
    let body = toml::to_string_pretty(graph).context("cannot write the pipeline as TOML")?;
    let mut out = String::new();
    if flags.paused {
        out.push_str("paused = true\n");
    }
    if flags.archived {
        out.push_str("archived = true\n");
    }
    out.push_str(&body);
    Ok(out)
}

/// The runnable at `reference` when it lives in a file this crate may change.
fn editable<'a>(catalog: &'a Catalog, reference: &RunnableRef) -> Result<&'a Runnable> {
    let runnable = catalog.get(reference).ok_or_else(|| anyhow!("`{reference}` does not exist"))?;
    check_writable(runnable)?;
    Ok(runnable)
}

fn check_writable(runnable: &Runnable) -> Result<()> {
    match &runnable.origin {
        Origin::Library | Origin::Project { .. } => Ok(()),
        Origin::Builtin => bail!("{} is built in and cannot be changed; duplicate it to edit a copy", runnable.name),
        Origin::ClaudeCode { path } => bail!(
            "{} is a Claude Code agent ({}) and is read-only here; duplicate it to edit a copy",
            runnable.name,
            path.display()
        ),
        Origin::Spawned { .. } => bail!("{} was created by another agent and cannot be edited", runnable.name),
    }
}

fn file_of(runnable: &Runnable) -> Result<&PathBuf> {
    runnable.path.as_ref().ok_or_else(|| anyhow!("{} has no file", runnable.name))
}

/// An id must be one safe file name.
fn check_id(id: &str) -> Result<()> {
    if id.is_empty() || id.starts_with('.') || id.contains(['/', '\\', '\0']) {
        bail!("`{id}` cannot be used as an id: it must be a plain file name");
    }
    Ok(())
}

pub(crate) fn slug(name: &str) -> String {
    base::id::slug(name, MAX_ID_LEN)
}

pub(crate) fn reference_of(kind: RunnableKind, id: &str) -> RunnableRef {
    match kind {
        RunnableKind::Agent => RunnableRef::agent(id),
        RunnableKind::Orchestrator => RunnableRef::orchestrator(id),
        RunnableKind::Pipeline => RunnableRef::pipeline(id),
    }
}

pub(crate) fn kind_word(kind: RunnableKind) -> &'static str {
    match kind {
        RunnableKind::Agent => "agent",
        RunnableKind::Orchestrator => "orchestrator",
        RunnableKind::Pipeline => "pipeline",
    }
}

/// Whether `id` is used by another definition of `kind`, in the catalog or as a file in `dir`.
pub(crate) fn id_taken(catalog: &Catalog, kind: RunnableKind, dir: &Path, id: &str) -> bool {
    catalog.get(&reference_of(kind, id)).is_some() || file_in(dir, kind, id).exists()
}

/// `base`, or `base-2`, `base-3`… when that is taken.
pub(crate) fn free_id(catalog: &Catalog, kind: RunnableKind, dir: &Path, base: &str) -> String {
    let mut id = base.to_string();
    let mut n = 2;
    while id_taken(catalog, kind, dir, &id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Writes `text` to `path` through a temp file in the same folder and a rename, creating the
/// folder when needed.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().ok_or_else(|| anyhow!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow!("cannot replace {}: {e}", path.display())
    })
}
