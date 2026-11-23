//! The built-in "Mockup → GPUI build" pipeline, written on first use. Its paths and models
//! come from the old single-pipeline config (`~/.config/harness/pipeline.toml`) when it exists.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::def::{Delegate, PipelineDef, StepDef, StepKind, Tools, WorkdirDef, config_dir, expand};

const TRIAGE_QUESTION: &str = "Does this issue need changes to the UI mockups before it is coded? \
Changes to what the user sees (new or changed controls, layout, copy, states, screens) need a \
mockup change; pure logic, data, performance, build or refactoring work does not. The mockups \
are static HTML screens in `screens/` of your workspace; read one only when the issue is \
ambiguous.";

const DESIGNER: &str = "You change the UI mockups for one issue. The mockups are static HTML \
screens in `screens/` of your workspace; `README.md` there explains the format and `GRAPHITE.md` \
the design system. Edit the HTML in place (edit_file for small changes); keep files static (no \
scripts, no network assets), keep the existing structure, change only what the issue needs and \
stay consistent with the surrounding screens. Do not touch unrelated screens. The user reviews \
renders of the screens you change before anything is coded.";

const ORCHESTRATOR: &str = "You lead coders to implement one issue in a Rust + GPUI codebase. \
You do not edit files yourself: plan the work, hand it to coders with `delegate` (one focused \
task at a time, with exact files and acceptance criteria), review what they did (read the code, \
`git diff`), and send follow-up tasks until it is right. Then verify: build, run the tests and \
the linter as the project instructions say, and delegate fixes until everything passes. Do not \
commit or push. If earlier steps changed mockups, they are approved: read them from the mockup \
workdir with `read_workdir` and implement them faithfully. Use `load_skill` for GPUI guidance.";

const CODER: &str = "You implement one task given by the orchestrator in a Rust + GPUI \
codebase. Read the relevant code first, make focused changes in the existing style (edit_file \
for small edits), and check your work (at least compile it as the project instructions say). \
Do not commit. Use `load_skill` for GPUI guidance and `read_workdir` for approved mockups.";

fn gpui_skills() -> Vec<String> {
    [
        "gpui-elements",
        "gpui-styling",
        "gpui-entities",
        "gpui-fundamentals",
        "gpui-patterns",
        "gpui-troubleshooting",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Values of the old `pipeline.toml` (paths, models, warm-up), if present.
struct Legacy {
    code_repo: Option<PathBuf>,
    design_dir: Option<PathBuf>,
    base_branch: String,
    design_branch: String,
    draft: bool,
    push: bool,
    warmup: Option<String>,
    target: Option<String>,
    models: BTreeMap<String, String>,
}

fn legacy() -> Legacy {
    let v: toml::Value = std::fs::read_to_string(config_dir().join("pipeline.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or(toml::Value::Table(Default::default()));
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool());
    let mut models = BTreeMap::new();
    if let Some(agents) = v.get("agents").and_then(|a| a.as_table()) {
        for (role, t) in agents {
            if let Some(m) = t.get("model").and_then(|m| m.as_str()) {
                models.insert(role.clone(), m.to_string());
            }
        }
    }
    Legacy {
        code_repo: s("code_repo").map(|p| expand(&p)),
        design_dir: s("design_dir").map(|p| expand(&p)),
        base_branch: s("base_branch").unwrap_or_else(|| "main".into()),
        design_branch: s("design_branch").unwrap_or_else(|| "main".into()),
        draft: b("draft_pr").unwrap_or(true),
        push: b("push").unwrap_or(true),
        warmup: s("warmup"),
        target: s("cargo_target_dir"),
        models,
    }
}

/// The "Mockup → GPUI build" pipeline: triage → mockup → render → your approval → merge
/// mockup → build (orchestrator + coders) → pull request.
pub fn legacy_default() -> PipelineDef {
    let l = legacy();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let code_repo = l
        .code_repo
        .clone()
        .or_else(|| std::env::var("HARNESS_WORKSPACE").ok().filter(|s| !s.is_empty()).map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let repo_name = code_repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let design_dir = l
        .design_dir
        .clone()
        .unwrap_or_else(|| home.join("Pictures/designs").join(format!("{repo_name}-mockups")));
    let model = |role: &str, default: &str| l.models.get(role).cloned().unwrap_or_else(|| default.to_string());
    let mut env = BTreeMap::new();
    let target = l
        .target
        .clone()
        .unwrap_or_else(|| format!("~/.harness/cargo-target/{repo_name}"));
    if !target.trim().is_empty() {
        env.insert("CARGO_TARGET_DIR".to_string(), expand(&target).display().to_string());
    }

    let workdirs = vec![
        WorkdirDef {
            id: "mockups".into(),
            name: "Mockups".into(),
            path: design_dir,
            worktree: true,
            base_branch: l.design_branch.clone(),
            branch_prefix: "design/".into(),
            setup: String::new(),
            env: BTreeMap::new(),
        },
        WorkdirDef {
            id: "code".into(),
            name: "Code".into(),
            path: code_repo,
            worktree: true,
            base_branch: l.base_branch.clone(),
            branch_prefix: String::new(),
            setup: l
                .warmup
                .clone()
                .unwrap_or_else(|| "nix-shell shell.nix --run 'cargo build' </dev/null".into()),
            env,
        },
    ];
    let steps = vec![
        StepDef {
            id: "triage".into(),
            name: "Triage".into(),
            kind: StepKind::Decision,
            workdir: "mockups".into(),
            model: model("triage", "nr/cc/claude-haiku-5-5"),
            tools: Tools::ReadOnly,
            max_steps: 12,
            question: TRIAGE_QUESTION.into(),
            on_no: "build".into(),
            ..StepDef::default()
        },
        StepDef {
            id: "mockup".into(),
            name: "Mockup".into(),
            kind: StepKind::Agent,
            workdir: "mockups".into(),
            model: model("designer", "nr/cc/claude-opus-5-5"),
            instructions: DESIGNER.into(),
            tools: Tools::Full,
            skills: vec!["ui-ux-pro-max".into()],
            max_steps: 60,
            ..StepDef::default()
        },
        StepDef {
            id: "render".into(),
            name: "Render mockups".into(),
            kind: StepKind::Render,
            workdir: "mockups".into(),
            files: "screens/*.html".into(),
            ..StepDef::default()
        },
        StepDef {
            id: "approval".into(),
            name: "Your approval".into(),
            kind: StepKind::Approval,
            back_to: "mockup".into(),
            skip_if_nothing_to_review: true,
            ..StepDef::default()
        },
        StepDef {
            id: "merge-mockup".into(),
            name: "Merge mockup".into(),
            kind: StepKind::Merge,
            workdir: "mockups".into(),
            ..StepDef::default()
        },
        StepDef {
            id: "build".into(),
            name: "Build".into(),
            kind: StepKind::Agent,
            workdir: "code".into(),
            model: model("orchestrator", "nr/cc/claude-opus-5-5"),
            instructions: ORCHESTRATOR.into(),
            tools: Tools::ReadAndRun,
            skills_on_demand: gpui_skills(),
            max_steps: 80,
            delegate: Some(Delegate {
                name: "Coder".into(),
                model: model("coder", "nr/cc/claude-sonnet-5-5"),
                tools: Tools::Full,
                skills: Vec::new(),
                skills_on_demand: gpui_skills(),
                max_steps: 80,
                instructions: CODER.into(),
            }),
            ..StepDef::default()
        },
        StepDef {
            id: "pull-request".into(),
            name: "Pull request".into(),
            kind: StepKind::PullRequest,
            workdir: "code".into(),
            draft: l.draft,
            push: l.push,
            ..StepDef::default()
        },
    ];
    PipelineDef {
        id: "mockup-gpui-build".into(),
        name: "Mockup → GPUI build".into(),
        description: "Triage decides whether the mockup changes; the designer edits it and you \
            approve the renders; an orchestrator has coders build it, tests it and opens a PR."
            .into(),
        workdirs,
        steps,
    }
}
