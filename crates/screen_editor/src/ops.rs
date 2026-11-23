//! Structural edits: switching, creating and deleting pipelines; adding, moving,
//! duplicating and deleting steps; adding and deleting workdirs and picking their folders.

use std::path::PathBuf;

use gpui::{Context, PathPromptOptions, SharedString, Window};
use pipeline::{Delegate, PipelineDef, StepDef, StepKind, WorkdirDef};

use crate::{Confirm, Field, PipelineEditorScreen, Tab};

/// Name a new workdir has until its folder is picked.
const NEW_WORKDIR: &str = "New workdir";

/// Remove step `id` and clear the jumps that pointed at it.
pub(crate) fn remove_step(def: &mut PipelineDef, id: &str) {
    def.steps.retain(|s| s.id != id);
    for s in def.steps.iter_mut() {
        if s.on_no == id {
            s.on_no.clear();
        }
        if s.back_to == id {
            s.back_to.clear();
        }
    }
}

/// Move step `id` by `by` places (clamped).
pub(crate) fn move_step(def: &mut PipelineDef, id: &str, by: isize) {
    let Some(i) = def.step_index(id) else { return };
    let j = (i as isize + by).clamp(0, def.steps.len() as isize - 1) as usize;
    if i != j {
        let s = def.steps.remove(i);
        def.steps.insert(j, s);
    }
}

/// A copy of step `id` right after it (new id, "… copy" name). Returns the new id.
pub(crate) fn duplicate_step(def: &mut PipelineDef, id: &str) -> Option<String> {
    let i = def.step_index(id)?;
    let mut s = def.steps[i].clone();
    s.id = def.unique_step_id(&s.id);
    s.name = format!("{} copy", s.name);
    let new = s.id.clone();
    def.steps.insert(i + 1, s);
    Some(new)
}

/// Steps that work in workdir `id`.
pub(crate) fn steps_using<'a>(def: &'a PipelineDef, id: &str) -> Vec<&'a StepDef> {
    def.steps
        .iter()
        .filter(|s| s.kind.uses_workdir() && s.workdir == id)
        .collect()
}

impl PipelineEditorScreen {
    // ── Pipelines ───────────────────────────────────────────────────────

    pub(crate) fn switch_pipeline(&mut self, id: String, cx: &mut Context<Self>) {
        self.commit_pending(cx);
        self.menu = None;
        self.confirm = None;
        self.error = None;
        self.sel = None;
        self.wd_sel = None;
        self.pid = Some(id);
        if self.tab == Tab::Step {
            self.tab = Tab::Pipeline;
        }
        self.load_inputs(cx);
        self.fitted = false;
        cx.notify();
    }

    /// Create a pipeline (a copy of `template`, or blank), open it and focus its name.
    pub(crate) fn new_pipeline(&mut self, template: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending(cx);
        let name = match template.as_deref().and_then(|t| self.pipes.read(cx).pipeline(t)) {
            Some(t) => format!("Copy of {}", t.name),
            None => "New pipeline".to_string(),
        };
        let made = self
            .pipes
            .update(cx, |p, cx| p.create_pipeline(&name, template.as_deref(), cx));
        match made {
            Ok(id) => {
                self.switch_pipeline(id, cx);
                self.tab = Tab::Pipeline;
                self.focus_field(Field::PipeName, window, cx);
            }
            Err(e) => self.error = Some(format!("Could not create the pipeline: {e:#}").into()),
        }
        self.menu = None;
        cx.notify();
    }

    pub(crate) fn delete_pipeline(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
        let Some(def) = self.def(cx) else { return };
        match self.pipes.update(cx, |p, cx| p.delete_pipeline(&def.id, cx)) {
            Ok(()) => {
                self.error = None;
                self.pid = None;
                self.sel = None;
                self.wd_sel = None;
                self.tab = Tab::Pipeline;
                self.load_inputs(cx);
                self.fitted = false;
            }
            Err(e) => {
                let msg = e.to_string();
                self.error = Some(
                    format!("Could not delete “{}”: {}.", def.name, if msg.is_empty() { "unknown error" } else { &msg })
                        .into(),
                );
            }
        }
        cx.notify();
    }

    // ── Steps ───────────────────────────────────────────────────────────

    /// Insert a new step of `kind` after the selected one (or at the end) and select it.
    pub(crate) fn add_step(&mut self, kind: StepKind, cx: &mut Context<Self>) {
        self.commit_pending(cx);
        let Some(def) = self.def(cx) else { return };
        let step = StepDef::new(kind, &def);
        let id = step.id.clone();
        let at = self
            .sel
            .as_deref()
            .and_then(|s| def.step_index(s))
            .map(|i| i + 1)
            .unwrap_or(def.steps.len());
        self.edit_def(cx, |d| d.steps.insert(at.min(d.steps.len()), step));
        self.menu = None;
        self.select(Some(id), cx);
    }

    pub(crate) fn move_selected(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(id) = self.sel.clone() else { return };
        self.commit_pending(cx);
        self.edit_def(cx, |d| move_step(d, &id, by));
    }

    pub(crate) fn duplicate_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.sel.clone() else { return };
        self.commit_pending(cx);
        let mut new = None;
        self.edit_def(cx, |d| new = duplicate_step(d, &id));
        if let Some(new) = new {
            self.select(Some(new), cx);
        }
    }

    pub(crate) fn delete_step(&mut self, id: String, cx: &mut Context<Self>) {
        self.confirm = None;
        self.edit_def(cx, |d| remove_step(d, &id));
        if self.sel.as_deref() == Some(id.as_str()) {
            self.sel = None;
            self.tab = Tab::Pipeline;
            self.load_inputs(cx);
        }
        cx.notify();
    }

    /// Change the selected step (toggles, dropdowns).
    pub(crate) fn edit_step(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut StepDef)) {
        let Some(id) = self.sel.clone() else { return };
        self.edit_def(cx, |d| {
            if let Some(s) = d.steps.iter_mut().find(|s| s.id == id) {
                f(s);
            }
        });
    }

    /// Turn sub-agents on (with defaults that follow the step) or off.
    pub(crate) fn toggle_delegate(&mut self, cx: &mut Context<Self>) {
        self.commit_pending(cx);
        self.edit_step(cx, |s| {
            s.delegate = match s.delegate.take() {
                Some(_) => None,
                None => Some(Delegate {
                    model: s.model.clone(),
                    skills_on_demand: s.skills_on_demand.clone(),
                    ..Delegate::default()
                }),
            };
        });
        self.load_inputs(cx);
    }

    pub(crate) fn ask_confirm(&mut self, c: Confirm, cx: &mut Context<Self>) {
        self.confirm = Some(c);
        cx.notify();
    }

    // ── Workdirs ────────────────────────────────────────────────────────

    pub(crate) fn expand_workdir(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.wd_sel == id {
            return;
        }
        self.commit_pending(cx);
        self.wd_sel = id;
        self.confirm = None;
        self.load_inputs(cx);
        cx.notify();
    }

    /// Add a workdir, expand it and ask for its folder.
    pub(crate) fn add_workdir(&mut self, cx: &mut Context<Self>) {
        self.commit_pending(cx);
        let Some(def) = self.def(cx) else { return };
        let id = def.unique_workdir_id(NEW_WORKDIR);
        let wd = WorkdirDef {
            id: id.clone(),
            name: NEW_WORKDIR.into(),
            worktree: false,
            ..WorkdirDef::default()
        };
        self.edit_def(cx, |d| d.workdirs.push(wd));
        self.tab = Tab::Workdirs;
        self.expand_workdir(Some(id.clone()), cx);
        self.pick_folder(id, cx);
    }

    pub(crate) fn delete_workdir(&mut self, id: String, cx: &mut Context<Self>) {
        self.confirm = None;
        self.edit_def(cx, |d| d.workdirs.retain(|w| w.id != id));
        if self.wd_sel.as_deref() == Some(id.as_str()) {
            self.wd_sel = None;
            self.load_inputs(cx);
        }
        cx.notify();
    }

    pub(crate) fn edit_workdir(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut WorkdirDef)) {
        self.edit_def(cx, |d| {
            if let Some(w) = d.workdirs.iter_mut().find(|w| w.id == id) {
                f(w);
            }
        });
    }

    /// Ask for a folder for workdir `id`. A workdir still called "New workdir" that no step
    /// uses takes the folder's name (and an id from it); worktrees are on for git repos.
    pub(crate) fn pick_folder(&mut self, id: String, cx: &mut Context<Self>) {
        self.menu = None;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked: Option<PathBuf> = match rx.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(e)) => {
                    let msg: SharedString = format!("Could not open the folder picker: {e:#}").into();
                    this.update(cx, |this, cx| {
                        this.error = Some(msg);
                        cx.notify();
                    })
                    .ok();
                    None
                }
                _ => None,
            };
            let Some(path) = picked else { return };
            this.update(cx, |this, cx| this.set_folder(&id, path, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    fn set_folder(&mut self, id: &str, path: PathBuf, cx: &mut Context<Self>) {
        let Some(def) = self.def(cx) else { return };
        let Some(w) = def.workdir(id) else { return };
        let fresh = w.name == NEW_WORKDIR && steps_using(&def, id).is_empty();
        let folder = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Folder".into());
        let is_git = path.join(".git").exists();
        let new_id = if fresh {
            let mut others = def.clone();
            others.workdirs.retain(|x| x.id != id);
            others.unique_workdir_id(&folder)
        } else {
            id.to_string()
        };
        let (old, nid) = (id.to_string(), new_id.clone());
        self.edit_def(cx, |d| {
            if let Some(w) = d.workdirs.iter_mut().find(|w| w.id == old) {
                w.path = path;
                if fresh {
                    w.name = capitalize(&folder);
                    w.id = nid;
                    w.worktree = is_git;
                }
            }
        });
        if self.wd_sel.as_deref() == Some(id) {
            self.wd_sel = Some(new_id);
        }
        self.load_inputs(cx);
        cx.notify();
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> PipelineDef {
        let mut d = pipeline::config::legacy_default();
        d.id = "t".into();
        d
    }

    #[test]
    fn deleting_a_step_clears_jumps_to_it() {
        let mut d = def();
        remove_step(&mut d, "mockup");
        assert!(d.step("mockup").is_none());
        let approval = d.step("approval").unwrap();
        assert!(approval.back_to.is_empty());
        remove_step(&mut d, "build");
        assert!(d.step("triage").unwrap().on_no.is_empty());
    }

    #[test]
    fn move_and_duplicate() {
        let mut d = def();
        move_step(&mut d, "triage", 1);
        assert_eq!(d.steps[1].id, "triage");
        move_step(&mut d, "triage", -5);
        assert_eq!(d.steps[0].id, "triage");
        let n = d.steps.len();
        let new = duplicate_step(&mut d, "build").unwrap();
        assert_eq!(d.steps.len(), n + 1);
        assert_ne!(new, "build");
        assert_eq!(d.step_index(&new), d.step_index("build").map(|i| i + 1));
        assert!(steps_using(&d, "code").len() >= 2);
        assert_eq!(capitalize("pipeliner"), "Pipeliner");
    }
}
