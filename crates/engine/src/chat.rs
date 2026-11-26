//! Chats: Direct (you and a model), Agent (a saved agent with tools),
//! Orchestrator (plan → your approval → sub-agents), Pipeline (starts a run).

use std::path::PathBuf;

use anyhow::{bail, Context as _, Result};
use harness_core::*;
use serde_json::{json, Value};

use crate::agent::{delegate_spec, AgentRun, Handled};
use crate::attachments;
use crate::llm::{self, Msg, ToolSpec};
use crate::prompt::{self, Stack};
use crate::tools::ToolContext;
use crate::Engine;

pub fn send(
    engine: &Engine,
    chat_id: &str,
    text: &str,
    attachments: Vec<Attachment>,
) -> Result<()> {
    let store = engine.store().clone();
    let mut chat = store.chat(chat_id).context("chat not found")?;
    let text = text.trim();
    if text.is_empty() && attachments.is_empty() {
        return Ok(());
    }

    // A task already working in this chat: the message steers it.
    if let Some(task) = chat.task_id.as_deref().and_then(|t| store.task(t)) {
        if task.status == Status::Running || task.status == Status::Waiting {
            let mut m = Message::new(chat_id, Role::User, text);
            m.author = "you".into();
            m.attachments = attachments;
            store.put_message(&m)?;
            // A waiting plan approval takes free text as feedback.
            if task.status == Status::Waiting {
                engine.decide(&task.id, DecisionKind::Reply { text: text.into() })?;
            }
            return Ok(());
        }
    }

    let mut m = Message::new(chat_id, Role::User, text);
    m.author = "you".into();
    m.delivered = true;
    m.attachments = attachments;

    // `/run <pipeline> <input>` starts a pipeline from any chat.
    if let Some(rest) = text.strip_prefix("/run ") {
        store.put_message(&m)?;
        let (name, input) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let p = store
            .pipelines()
            .into_iter()
            .find(|p| {
                p.name.eq_ignore_ascii_case(name)
                    || crate::git::slug(&p.name) == crate::git::slug(name)
            })
            .with_context(|| format!("no pipeline named `{name}`"))?;
        return start_pipeline(engine, &mut chat, &p.id, input);
    }
    // `@Agent text` switches the chat to that agent.
    if let Some(rest) = text.strip_prefix('@') {
        let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        if let Some(a) = store.agent_by_name(&name) {
            chat.mode = ChatMode::Agent;
            chat.agent_id = Some(a.id.clone());
        }
    }
    store.put_message(&m)?;
    chat.updated_at = now();
    store.put_chat(&chat)?;

    if chat.mode == ChatMode::Pipeline {
        let pid = chat
            .pipeline_id
            .clone()
            .context("pick a pipeline for this chat")?;
        return start_pipeline(engine, &mut chat, &pid, text);
    }

    let task_id = ensure_task(engine, &mut chat)?;
    // Mark busy now so a quick second message steers instead of starting a second reply.
    engine.store().update_task(&task_id, |t| {
        t.status = Status::Running;
        t.started_at.get_or_insert(now());
        t.finished_at = None;
        t.error = None;
    })?;
    maybe_title(
        engine,
        &chat,
        if text.is_empty() { "Screenshot" } else { text },
    );
    let engine = engine.clone();
    let chat_id = chat_id.to_string();
    std::thread::Builder::new()
        .name(format!("chat-{chat_id}"))
        .spawn(move || {
            // Chats are interactive: they don't take pipeline agent slots.
            let res = reply(&engine, &chat_id, &task_id);
            engine.drop_cancel_flag(&task_id);
            let _ = engine.store().update_task(&task_id, |t| {
                if t.status == Status::Cancelled {
                    return;
                }
                match &res {
                    Ok(s) => {
                        t.status = Status::Done;
                        t.summary = s.chars().take(300).collect();
                    }
                    Err(e) => {
                        t.status = Status::Failed;
                        t.error = Some(format!("{e:#}"));
                    }
                }
                t.finished_at = Some(now());
            });
            if let Err(e) = res {
                if format!("{e:#}") != "cancelled" {
                    let m = Message::new(&chat_id, Role::System, format!("Failed: {e:#}"));
                    let _ = engine.store().put_message(&m);
                }
            }
        })?;
    Ok(())
}

fn start_pipeline(engine: &Engine, chat: &mut Chat, pipeline_id: &str, input: &str) -> Result<()> {
    if input.trim().is_empty() {
        bail!("describe the task after the pipeline name");
    }
    let run = engine.start_run(pipeline_id, input, Some(&chat.id))?;
    let mut m = Message::new(
        &chat.id,
        Role::System,
        format!("Started run #{} · {}", run.number, run.title),
    );
    m.card = Some(Card::Run {
        run_id: run.id.clone(),
    });
    engine.store().put_message(&m)?;
    if chat.title == "New chat" {
        chat.title = run.title.chars().take(60).collect();
    }
    chat.updated_at = now();
    engine.store().put_chat(chat)?;
    Ok(())
}

/// Every chat has one task that accrues its cost and shows on the Board while it works.
fn ensure_task(engine: &Engine, chat: &mut Chat) -> Result<Id> {
    let store = engine.store();
    if let Some(id) = chat.task_id.clone().filter(|id| store.task(id).is_some()) {
        let agent = chat.agent_id.as_deref().and_then(|a| store.agent(a));
        store.update_task(&id, |t| {
            t.title = chat.title.clone();
            t.model = model_for(engine, chat);
            t.actor = actor_for(chat, agent.as_ref());
        })?;
        return Ok(id);
    }
    let agent = chat.agent_id.as_deref().and_then(|a| store.agent(a));
    let mut t = Task::new(TaskKind::Chat, chat.title.clone());
    t.chat_id = Some(chat.id.clone());
    t.actor = actor_for(chat, agent.as_ref());
    t.model = model_for(engine, chat);
    t.cwd = chat.cwd.clone();
    store.put_task(&t)?;
    chat.task_id = Some(t.id.clone());
    store.put_chat(chat)?;
    Ok(t.id)
}

fn actor_for(chat: &Chat, agent: Option<&AgentDef>) -> String {
    match chat.mode {
        ChatMode::Agent => agent
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Agent".into()),
        ChatMode::Orchestrator => "Orchestrator".into(),
        _ => Config::short_model(&chat.model),
    }
}

fn model_for(engine: &Engine, chat: &Chat) -> String {
    if chat.mode == ChatMode::Agent {
        if let Some(a) = chat
            .agent_id
            .as_deref()
            .and_then(|a| engine.store().agent(a))
        {
            return a.model;
        }
    }
    if chat.model.is_empty() {
        engine.config().default_model
    } else {
        chat.model.clone()
    }
}

/// Names the chat after its first message (fast model, in the background).
fn maybe_title(engine: &Engine, chat: &Chat, text: &str) {
    if chat.title != "New chat" {
        return;
    }
    let engine = engine.clone();
    let chat_id = chat.id.clone();
    let text = text.chars().take(1500).collect::<String>();
    std::thread::spawn(move || {
        let config = engine.config();
        let fallback: String = text
            .lines()
            .next()
            .unwrap_or("Chat")
            .chars()
            .take(48)
            .collect();
        let title = config
            .resolve(config.fast())
            .and_then(|(p, m)| {
                llm::quick(p, &m, "Write a 3-6 word title for this request. Reply with the title only, no quotes, no period.", &text).ok()
            })
            .map(|(t, _, _)| t.lines().next().unwrap_or("").trim_matches('"').to_string())
            .filter(|t| !t.is_empty() && t.len() < 80)
            .unwrap_or(fallback);
        if let Some(mut c) = engine.store().chat(&chat_id) {
            if c.title == "New chat" {
                c.title = title.clone();
                let _ = engine.store().put_chat(&c);
                if let Some(t) = c.task_id {
                    let _ = engine.store().update_task(&t, |t| t.title = title);
                }
            }
        }
    });
}

/// History of the chat as model messages, excluding the newest user turn, which is
/// returned separately as its text and images.
fn history(store: &Store, chat_id: &str) -> (Vec<Msg>, String, Vec<llm::ImageData>) {
    let mut msgs = store.messages(chat_id);
    msgs.retain(|m| {
        !m.partial
            && matches!(m.role, Role::User | Role::Assistant)
            && (!m.content.trim().is_empty() || (m.role == Role::User && !m.attachments.is_empty()))
    });
    let last_user = msgs.iter().rposition(|m| m.role == Role::User);
    let mut out = vec![];
    let mut latest = String::new();
    let mut latest_images = vec![];
    for (i, m) in msgs.iter().enumerate() {
        if m.role == Role::User {
            let (text, images) = attachments::images_for(&m.attachments, m.content.clone());
            if Some(i) == last_user {
                latest = text;
                latest_images = images;
            } else if images.is_empty() {
                out.push(Msg::User(text));
            } else {
                out.push(Msg::UserImages { text, images });
            }
            continue;
        }
        out.push(Msg::Assistant {
            content: m.content.clone(),
            tool_calls: vec![],
        });
    }
    (out, latest, latest_images)
}

fn reply(engine: &Engine, chat_id: &str, task_id: &str) -> Result<String> {
    let store = engine.store().clone();
    let chat = store.chat(chat_id).context("chat not found")?;
    let config = engine.config();
    let (hist, mut latest, images) = history(&store, chat_id);
    if latest.trim().is_empty() && !images.is_empty() {
        latest = "See the attached image.".into();
    }
    let cwd = chat
        .cwd
        .as_ref()
        .map(|c| harness_core::config::expand_tilde(c))
        .filter(|p| p.is_dir());
    let cancel = engine.cancel_flag(task_id);

    let mut stack = Stack {
        rules: config.rules.clone(),
        message: latest,
        images,
        ..Default::default()
    };
    let mut tool_ctx = ToolContext {
        cwd: cwd.clone(),
        shell: shell_for(cwd.as_deref()),
        env: Default::default(),
        access: ToolAccess::None,
        workdirs: vec![],
        skills_on_demand: vec![],
        config: config.clone(),
        cancel,
    };
    let mut actor = Config::short_model(&chat.model);
    let mut model = model_for(engine, &chat);
    let mut max_steps = 60;
    let mut extra: Vec<ToolSpec> = vec![];

    match chat.mode {
        ChatMode::Direct | ChatMode::Pipeline => {
            stack.role = "You are a helpful assistant in Pipeliner, a desktop app for running AI agent pipelines. \
                          Answer directly. Use Markdown for code."
                .into();
            if cwd.is_some() {
                tool_ctx.access = ToolAccess::ReadOnly;
            }
        }
        ChatMode::Agent => {
            let a = chat
                .agent_id
                .as_deref()
                .and_then(|a| store.agent(a))
                .context("pick an agent for this chat")?;
            stack.role = a.role.clone();
            stack.skills = a.skills.clone();
            stack.skills_on_demand = a.skills_on_demand.clone();
            tool_ctx.access = a.tools;
            tool_ctx.skills_on_demand = a.skills_on_demand.clone();
            actor = a.name.clone();
            model = a.model.clone();
            max_steps = a.max_steps;
        }
        ChatMode::Orchestrator => {
            stack.role = ORCHESTRATOR_ROLE.into();
            tool_ctx.access = ToolAccess::ReadOnly;
            let all: Vec<String> = engine.skills().into_iter().map(|s| s.name).collect();
            tool_ctx.skills_on_demand = all.clone();
            stack.skills_on_demand = all;
            actor = "Orchestrator".into();
            extra.push(ToolSpec {
                name: "propose_plan",
                description: "Show the user your plan and wait for approval. Returns 'approved' or their feedback.".into(),
                parameters: json!({"type":"object","properties":{"steps":{"type":"array","items":{"type":"string"}}},"required":["steps"]}),
            });
            extra.push(delegate_spec("worker"));
            max_steps = 80;
        }
    }
    stack.environment = prompt::environment(cwd.as_deref(), &tool_ctx.shell, &[]);
    if tool_ctx.access == ToolAccess::None {
        tool_ctx.cwd = None;
    }

    let run = AgentRun {
        engine,
        task_id: task_id.to_string(),
        thread_id: chat_id.to_string(),
        actor,
        model: model.clone(),
        stack,
        tools: tool_ctx,
        extra_tools: extra,
        history: hist,
        max_steps,
    };
    let mut handler = |run: &AgentRun, call: &llm::Call, args: &Value| -> Option<Handled> {
        match call.name.as_str() {
            "propose_plan" => Some(propose_plan(run, args)),
            "delegate" => Some(delegate(
                run,
                args,
                &run.model,
                "Worker",
                DELEGATE_ROLE,
                &[],
                60,
            )),
            _ => None,
        }
    };
    let out = run.run(&mut handler)?;
    Ok(out.text)
}

pub fn shell_for(cwd: Option<&std::path::Path>) -> String {
    match cwd {
        Some(c) if c.join("shell.nix").is_file() => "nix-shell shell.nix --run".into(),
        Some(c) if c.join("flake.nix").is_file() => "nix develop --command sh -c".into(),
        _ => String::new(),
    }
}

const ORCHESTRATOR_ROLE: &str = "You are an orchestrator. First understand the request (read files if a folder is attached), \
then call propose_plan with short concrete steps and wait for approval. After approval, hand each step to a worker with \
`delegate` (exact files, change, acceptance criteria), review their reports, and send follow-ups until done. \
Finish with a short summary of what changed and what is left.";

pub const DELEGATE_ROLE: &str = "You do one focused task given by an orchestrator. Read the relevant code first, make focused \
changes in the existing style (edit_file for small edits), check your work (build/test as the project instructions say), \
and reply with a short report: what you changed (files), how you checked it, anything left. Do not commit.";

fn propose_plan(run: &AgentRun, args: &Value) -> Handled {
    let store = run.engine.store();
    let steps: Vec<String> = args["steps"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if steps.is_empty() {
        return Handled::Continue("Error: steps is empty".into());
    }
    let mut m = Message::new(&run.thread_id, Role::System, "Plan ready for your approval");
    m.card = Some(Card::Plan {
        task_id: run.task_id.clone(),
        steps: steps.clone(),
    });
    let _ = store.put_message(&m);
    // Stale decisions (e.g. a double click on an earlier plan) must not answer this one.
    while store.take_decision(&run.task_id).is_some() {}
    let _ = store.update_task(&run.task_id, |t| {
        t.status = Status::Waiting;
        t.summary = "Plan waiting for your approval".into();
    });
    store.log(
        "plan.proposed",
        None,
        Some(&run.task_id),
        format!("Plan with {} steps waiting for you", steps.len()),
    );
    let cancel = run.tools.cancel.clone();
    let decision = loop {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Handled::Stop("cancelled".into(), Value::Null);
        }
        if let Some(d) = store.take_decision(&run.task_id) {
            break d;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    };
    let _ = store.update_task(&run.task_id, |t| {
        t.status = Status::Running;
        t.summary.clear();
    });
    // Free-text feedback was already posted as a chat message; don't repeat it as steering.
    let _ = store.take_undelivered(&run.thread_id);
    match decision.kind {
        DecisionKind::Approve | DecisionKind::Pick { .. } => {
            Handled::Continue("approved — carry out the plan".into())
        }
        DecisionKind::Reply { text } | DecisionKind::SendBack { note: text, .. } => {
            Handled::Continue(format!(
                "not approved yet; user feedback: {text}\nRevise and propose again."
            ))
        }
        DecisionKind::Reject { reason } => {
            Handled::Stop(format!("rejected: {reason}"), json!({"rejected": true}))
        }
    }
}

/// Runs a sub-agent as a spawned task (depth + 1) and returns its report.
pub fn delegate(
    run: &AgentRun,
    args: &Value,
    model: &str,
    name: &str,
    role: &str,
    skills_on_demand: &[String],
    max_steps: u32,
) -> Handled {
    let engine = run.engine;
    let store = engine.store();
    let title = args["title"].as_str().unwrap_or("Sub-task").to_string();
    let task_text = args["task"].as_str().unwrap_or("").to_string();
    if task_text.trim().is_empty() {
        return Handled::Continue("Error: task is empty".into());
    }
    let parent = store.task(&run.task_id);
    let mut t = Task::new(TaskKind::Spawned, title.clone());
    t.parent_id = Some(run.task_id.clone());
    t.depth = parent.as_ref().map(|p| p.depth + 1).unwrap_or(1);
    t.run_id = parent.as_ref().and_then(|p| p.run_id.clone());
    t.chat_id = parent.as_ref().and_then(|p| p.chat_id.clone());
    t.actor = name.into();
    t.model = model.into();
    t.cwd = run.tools.cwd.as_ref().map(|p| p.display().to_string());
    t.status = Status::Running;
    t.started_at = Some(now());
    if store.put_task(&t).is_err() {
        return Handled::Continue("Error: could not create the sub-task".into());
    }
    store.log(
        "task.spawned",
        t.run_id.as_deref(),
        Some(&t.id),
        format!("{} spawned {title}", run.actor),
    );
    let mut first = Message::new(&t.id, Role::User, task_text.clone());
    first.author = run.actor.clone();
    first.delivered = true;
    let _ = store.put_message(&first);

    let cancel = engine.cancel_flag(&t.id);
    let cwd: Option<PathBuf> = run.tools.cwd.clone();
    let sub = AgentRun {
        engine,
        task_id: t.id.clone(),
        thread_id: t.id.clone(),
        actor: name.into(),
        model: model.into(),
        stack: Stack {
            rules: engine.config().rules,
            role: role.into(),
            environment: prompt::environment(cwd.as_deref(), &run.tools.shell, &run.tools.workdirs),
            skills_on_demand: skills_on_demand.to_vec(),
            message: task_text,
            ..Default::default()
        },
        tools: ToolContext {
            cwd,
            shell: run.tools.shell.clone(),
            env: run.tools.env.clone(),
            access: ToolAccess::Full,
            workdirs: run.tools.workdirs.clone(),
            skills_on_demand: skills_on_demand.to_vec(),
            config: engine.config(),
            cancel,
        },
        extra_tools: vec![],
        history: vec![],
        max_steps,
    };
    let res = sub.run(&mut |_, _, _| None);
    engine.drop_cancel_flag(&t.id);
    let (status, text) = match res {
        Ok(o) => (Status::Done, o.text),
        Err(e) => (Status::Failed, format!("Sub-agent failed: {e:#}")),
    };
    let _ = store.update_task(&t.id, |t| {
        if t.status != Status::Cancelled {
            t.status = status;
        }
        t.summary = text.chars().take(600).collect();
        t.finished_at = Some(now());
    });
    Handled::Continue(format!("Report from {name} ({title}):\n{text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_builds_user_images() {
        let dir = std::env::temp_dir().join(format!("pipeliner-chat-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&4u32.to_be_bytes());
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        std::fs::write(&file, &png).unwrap();
        let att = |path: &std::path::Path, name: &str| Attachment {
            path: path.to_string_lossy().into_owned(),
            name: name.into(),
            mime: "image/png".into(),
            width: 4,
            height: 3,
            pasted: false,
        };

        let store = Store::open_in_memory().unwrap();
        let mut m1 = Message::new("c1", Role::User, "what is this?");
        m1.attachments = vec![att(&file, "a.png"), att(&dir.join("gone.png"), "gone.png")];
        let m2 = Message::new("c1", Role::Assistant, "a tiny image");
        let mut m3 = Message::new("c1", Role::User, "");
        m3.attachments = vec![att(&file, "a.png")];
        for m in [&m1, &m2, &m3] {
            store.put_message(m).unwrap();
        }

        let (hist, latest, images) = history(&store, "c1");
        assert_eq!(latest, "");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime, "image/png");
        assert_eq!(hist.len(), 2);
        match &hist[0] {
            Msg::UserImages { text, images } => {
                assert_eq!(images.len(), 1);
                assert_eq!(text, "what is this?\n[image gone.png is no longer on disk]");
            }
            other => panic!("expected UserImages, got {other:?}"),
        }
        assert!(matches!(hist[1], Msg::Assistant { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }
}
