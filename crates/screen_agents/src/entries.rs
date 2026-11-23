//! The card list: every pipeline, the model steps and sub-agents of each, the chat agents,
//! then the tasks sub-agents worked on.

use gpui::SharedString;
use pipeline::{StageStatus, StepKind};
use ui::nav::{ChatSessionInfo, ChatStatus};

use crate::fmt;
use crate::model::{
    Catalog, ChatKind, Entry, Kind, Snapshot, State, StepRef, StepRole, Usage, plural,
    source_label, stage_word,
};

/// Chats with an agent (from the chat screen's session list) → state + status text.
fn chat_state(name: &str, chats: &[ChatSessionInfo]) -> (State, String, usize) {
    let mine: Vec<&ChatSessionInfo> = chats.iter().filter(|c| c.agent == name).collect();
    let running = mine
        .iter()
        .filter(|c| c.status == ChatStatus::Running)
        .count();
    let waiting = mine
        .iter()
        .filter(|c| c.status == ChatStatus::Waiting)
        .count();
    let (state, text) = if running > 0 {
        (
            State::Running,
            format!("{running} chat{} running", plural(running)),
        )
    } else if waiting > 0 {
        (State::Waiting, "Waiting for you in chat".into())
    } else {
        (State::Idle, "Idle".into())
    };
    (state, text, mine.len())
}

fn recent_chats(n: usize) -> String {
    if n == 0 {
        "No recent chats".into()
    } else {
        format!("{n} recent chat{}", plural(n))
    }
}

/// `3 runs · $1.20`, `2 runs`, `No runs yet`.
fn usage_meta(u: &Usage, runs: usize) -> String {
    if u.runs > 0 {
        let spent = if u.cost > 0. {
            fmt::cost(u.cost)
        } else {
            format!("{} tokens", fmt::tokens(u.tokens))
        };
        format!("{} run{} · {spent}", u.runs, plural(u.runs))
    } else if runs > 0 {
        format!("{runs} run{}", plural(runs))
    } else {
        "No runs yet".into()
    }
}

/// Entry ids: `pipeline-<id>`, `step-<pipeline>-<step>`, `sub-<pipeline>-<step>`.
pub fn pipeline_id(id: &str) -> SharedString {
    format!("pipeline-{id}").into()
}

pub fn step_id(pipeline: &str, step: &str) -> SharedString {
    format!("step-{pipeline}-{step}").into()
}

pub fn sub_id(pipeline: &str, step: &str) -> SharedString {
    format!("sub-{pipeline}-{step}").into()
}

pub fn entries(snap: &Snapshot, cat: &Catalog, chats: &[ChatSessionInfo]) -> Vec<Entry> {
    let mut out = Vec::new();

    // Pipelines.
    for (pi, def) in snap.pipelines.iter().enumerate() {
        let runs = snap.runs_in(&def.id).len();
        let active = snap.active_in(&def.id);
        let waiting = snap.waiting_in(&def.id);
        let status = if active > 0 {
            format!("{active} running")
        } else if waiting > 0 {
            format!("{waiting} waiting for approval")
        } else {
            "Idle".into()
        };
        let meta = match snap.last_run_in(&def.id) {
            Some(t) => format!("{runs} run{} · last {}", plural(runs), fmt::when(t)),
            None => "No runs yet".into(),
        };
        let n = def.steps.len();
        let mut chips: Vec<SharedString> = vec![format!("{n} step{}", plural(n)).into()];
        let w = def.workdirs.len();
        if w > 0 {
            chips.push(format!("{w} workdir{}", plural(w)).into());
        }
        let cost = snap.cost_in(&def.id);
        if cost > 0. {
            chips.push(fmt::cost(cost).into());
        }
        let desc = if def.description.trim().is_empty() {
            def.steps
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(" › ")
        } else {
            def.description.trim().to_string()
        };
        out.push(Entry {
            id: pipeline_id(&def.id),
            name: def.name.clone().into(),
            kind: Kind::Pipeline(pi),
            desc: if desc.is_empty() { "No steps yet".into() } else { desc.into() },
            state: snap.pipeline_state(&def.id),
            status: status.into(),
            meta: meta.into(),
            chips,
        });
    }

    // Model steps of each pipeline, then the sub-agents they lead.
    let mut subs = Vec::new();
    for (pi, def) in snap.pipelines.iter().enumerate() {
        for (si, step) in def.steps.iter().enumerate() {
            if !step.kind.uses_model() {
                continue;
            }
            let r = StepRef { pipeline: pi, step: si };
            let role = StepRole::of(step);
            let (state, status) = match snap.running_step(r).first() {
                Some(run) => (State::Running, format!("Running on {}", run.issue)),
                None => (State::Idle, "Idle".to_string()),
            };
            let desc = match step.kind {
                StepKind::Decision if !step.question.trim().is_empty() => step.question.trim(),
                _ => step.instructions.trim(),
            };
            let mut chips: Vec<SharedString> = vec![fmt::short_model(&step.model).to_string().into()];
            if let Some(w) = def.workdir(&step.workdir) {
                chips.push(w.name.clone().into());
            }
            out.push(Entry {
                id: step_id(&def.id, &step.id),
                name: step.name.clone().into(),
                kind: Kind::Step(r, role),
                desc: if desc.is_empty() {
                    format!("{} in {}", step.kind.label(), def.name).into()
                } else {
                    fmt::plain_line(desc).into()
                },
                state,
                status: status.into(),
                meta: format!(
                    "{} · {}",
                    def.name,
                    usage_meta(&snap.step_usage(r), snap.runs_of_step(r).len())
                )
                .into(),
                chips,
            });
            if let Some(d) = &step.delegate {
                let (state, status) = match snap.running_delegate(r).first() {
                    Some(run) => (State::Running, format!("Running on {}", run.issue)),
                    None => (State::Idle, "Idle".to_string()),
                };
                let instr = fmt::plain_line(&d.instructions);
                subs.push(Entry {
                    id: sub_id(&def.id, &step.id),
                    name: d.name.clone().into(),
                    kind: Kind::Delegate(r),
                    desc: if instr.is_empty() {
                        format!("Takes tasks from {}", step.name).into()
                    } else {
                        instr.into()
                    },
                    state,
                    status: status.into(),
                    meta: format!(
                        "Led by {} · {}",
                        step.name,
                        usage_meta(&snap.delegate_usage(r), snap.runs_of_delegate(r).len())
                    )
                    .into(),
                    chips: vec![
                        fmt::short_model(&d.model).to_string().into(),
                        d.tools.label().into(),
                    ],
                });
            }
        }
    }
    out.extend(subs);

    // Chat agents.
    for kind in ChatKind::ALL {
        let (state, status, n) = chat_state(kind.name(), chats);
        out.push(Entry {
            id: format!("chat-{}", kind.name().to_lowercase()).into(),
            name: kind.name().into(),
            kind: Kind::Chat(kind),
            desc: kind.hint().into(),
            state,
            status: status.into(),
            meta: recent_chats(n).into(),
            chips: vec!["Built-in".into()],
        });
    }
    for (i, a) in cat.agents.iter().enumerate() {
        let (state, status, n) = chat_state(&a.name, chats);
        let tools = match &a.tools {
            None => "Default tools".to_string(),
            Some(t) => format!("{} tool{}", t.len(), plural(t.len())),
        };
        let mut chips: Vec<SharedString> = Vec::new();
        if let Some(m) = &a.model {
            chips.push(m.clone().into());
        }
        chips.push(tools.into());
        out.push(Entry {
            id: format!("custom-{}", a.name).into(),
            name: a.name.clone().into(),
            kind: Kind::Custom(i),
            desc: if a.description.is_empty() {
                "No description".into()
            } else {
                a.description.clone().into()
            },
            state,
            status: status.into(),
            meta: format!("{} · {}", source_label(&a.source), recent_chats(n)).into(),
            chips,
        });
    }

    // Tasks sub-agents worked on.
    for ix in 0..snap.tasks.len() {
        let Some((run, task)) = snap.task(ix) else {
            continue;
        };
        let state = match State::of_stage(task.status) {
            // A task left "running" by a run that is no longer active did not finish.
            State::Running if !run.active => State::Idle,
            s => s,
        };
        let status = if state == State::Idle && task.status == StageStatus::Running {
            "Stopped"
        } else {
            stage_word(task.status)
        };
        let summary = fmt::plain_line(&task.summary);
        out.push(Entry {
            id: format!("task-{}-{}", run.id, snap.tasks[ix].index).into(),
            name: task.title.clone().into(),
            kind: Kind::Task(ix),
            desc: if summary.is_empty() {
                "No summary yet".into()
            } else {
                summary.into()
            },
            state,
            status: status.into(),
            meta: format!("{} · {}", run.issue, fmt::when(run.created)).into(),
            chips: snap
                .task_agent(ix)
                .map(|c| vec![c.into()])
                .unwrap_or_default(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pipeline::{AgentUsage, CoderTask, RunStatus};

    use super::*;
    use crate::model::{Filter, RunInfo, StageInfo, is_delegate};

    fn run(issue: &str, active: bool, status: RunStatus) -> RunInfo {
        let def = pipeline::config::legacy_default();
        RunInfo {
            issue: issue.into(),
            title: format!("{issue} title"),
            id: format!("run-{issue}"),
            pipeline: "mockup-gpui-build".into(),
            status,
            active,
            created: 100,
            tokens: 0,
            cost: 0.,
            usage: Vec::new(),
            stages: def
                .steps
                .iter()
                .map(|s| StageInfo {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    status: StageStatus::Pending,
                })
                .collect(),
            tasks: Vec::new(),
            delegates: vec![("build".into(), "Coder".into())],
        }
    }

    fn usage(who: &str, tokens: u64, cost: f64) -> AgentUsage {
        AgentUsage {
            who: who.into(),
            model: "p/m".into(),
            tokens,
            cost_usd: cost,
        }
    }

    fn task(title: &str, status: StageStatus, summary: &str) -> CoderTask {
        CoderTask {
            title: title.into(),
            status,
            summary: summary.into(),
            step: "build".into(),
        }
    }

    #[test]
    fn delegate_names() {
        assert!(is_delegate("Coder 12", "Coder"));
        assert!(is_delegate("Coder", "Coder"));
        assert!(!is_delegate("Coderx", "Coder"));
        assert!(!is_delegate("Coder x", "Coder"));
    }

    #[test]
    fn usage_and_tasks_come_from_runs() {
        let mut def = pipeline::config::legacy_default();
        def.id = "mockup-gpui-build".into();
        // Steps: triage, mockup, render, approval, merge-mockup, build, pull-request.
        let build = StepRef { pipeline: 0, step: 5 };
        let triage = StepRef { pipeline: 0, step: 0 };

        let mut a = run("HAR-2", true, RunStatus::Running);
        a.usage = vec![usage("Build", 10, 0.5), usage("Coder 1", 5, 0.25)];
        a.stages[5].status = StageStatus::Running;
        a.tasks = vec![task("Do it", StageStatus::Running, "")];
        let mut b = run("HAR-1", false, RunStatus::Done);
        b.usage = vec![usage("Coder 1", 1, 0.), usage("Coder 2", 1, 0.)];
        b.tasks = vec![
            task("One", StageStatus::Done, "- **Done**"),
            task("Two", StageStatus::Failed, ""),
        ];
        let snap = Snapshot::from_runs(vec![def], Vec::new(), vec![a, b], PathBuf::new());

        let coders = snap.delegate_usage(build);
        assert_eq!((coders.runs, coders.tokens), (2, 7));
        assert_eq!(snap.step_usage(build).tokens, 10);
        assert_eq!(snap.step_usage(triage), Usage::default());
        assert_eq!(snap.running_step(build).len(), 1);
        assert_eq!(snap.running_delegate(build).len(), 1);
        assert!(snap.running_step(triage).is_empty());
        assert_eq!(snap.pipeline_state("mockup-gpui-build"), State::Running);

        assert_eq!(snap.tasks.len(), 3);
        assert_eq!(snap.task_agent(2).as_deref(), Some("Coder 2"));
        assert_eq!(snap.task_lead(2).as_deref(), Some("Build"));
        assert_eq!(snap.task_usage(2).len(), 1);
        assert_eq!(snap.tasks_of(build).len(), 3);

        let cat = Catalog {
            root: PathBuf::new(),
            agents: Vec::new(),
            skills: Vec::new(),
        };
        let list = entries(&snap, &cat, &[]);
        let count = |f: Filter| list.iter().filter(|e| f.matches(e)).count();
        assert_eq!(count(Filter::Pipelines), 1);
        // Triage, Mockup, Build.
        assert_eq!(count(Filter::Steps), 3);
        assert_eq!(count(Filter::SubAgents), 1);
        assert_eq!(count(Filter::ChatAgents), 2);
        assert_eq!(count(Filter::Created), 3);
        let one = list.iter().find(|e| e.name == "One").unwrap();
        assert_eq!(one.desc.as_ref(), "Done");
        assert_eq!(one.state, State::Done);
        let lead = list.iter().find(|e| e.id == step_id("mockup-gpui-build", "build")).unwrap();
        assert_eq!(lead.kind, Kind::Step(build, StepRole::Lead));
        assert_eq!(lead.state, State::Running);
    }
}
