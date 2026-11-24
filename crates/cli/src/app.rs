//! The commands, over an [`Engine`] the caller opens. `main` passes the real terminal and
//! engine; tests pass buffers and an engine on a scripted model.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base::paths::Paths;
use engine::Engine;
use engine::events::{Receiver, TryRecvError};
use engine::api::{
    AgentEvent, Api, ChatApi, Event, OpsApi, ModelRef, NewSession, NoticeLevel, ParentFilter, PermissionDecision,
    PermissionRequest, SessionFilter, SessionId, SessionState, UserMessage, WorkMode, WorkdirChoice,
};

use crate::args::{self, Args, ChatArgs, Command, DATA_DIR_ENV, USAGE};
use crate::render::{self, Printer};
use crate::signal;

/// How often the turn loop looks for events and Ctrl-C.
const POLL: Duration = Duration::from_millis(10);

/// A lookup of environment variables.
pub type EnvLookup = Box<dyn Fn(&str) -> Option<String>>;

/// What the commands run against.
pub struct Deps {
    /// Opens the engine over the resolved paths.
    pub open: Box<dyn Fn(Paths) -> Result<Engine>>,
    /// Lines typed by the person: messages and answers to permission questions.
    pub input: Box<dyn BufRead>,
    /// Answers and tool lines.
    pub out: Box<dyn Write>,
    /// Prompts, questions, errors.
    pub err: Box<dyn Write>,
    /// Where a chat works without `--dir`.
    pub cwd: PathBuf,
    /// Reads `DATA_DIR_ENV`; the process environment in `main`.
    pub env: EnvLookup,
}

/// Runs the command line and returns the exit code.
pub fn run(argv: &[String], mut deps: Deps) -> i32 {
    let parsed = match args::parse(argv) {
        Ok(parsed) => parsed,
        Err(error) => {
            let _ = writeln!(deps.err, "harness-cli: {error:#}");
            return 2;
        }
    };
    match execute(parsed, &mut deps) {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(deps.err, "harness-cli: {error:#}");
            1
        }
    }
}

fn execute(args: Args, deps: &mut Deps) -> Result<i32> {
    if args.command == Command::Help {
        write!(deps.out, "{USAGE}")?;
        return Ok(0);
    }
    let data_dir = args.data_dir.or_else(|| (deps.env)(DATA_DIR_ENV).filter(|d| !d.is_empty()).map(PathBuf::from));
    let paths = match data_dir {
        Some(dir) => Paths::under(&dir),
        None => Paths::from_env(),
    };
    let engine = (deps.open)(paths)?;
    let result = match args.command {
        Command::Chat(chat_args) => chat(&engine, chat_args, deps),
        Command::Sessions => sessions(&engine, deps),
        Command::Models => models(&engine, deps),
        Command::Import { dry_run } => {
            let report = engine.import_v1(dry_run)?;
            write!(deps.out, "{report}")?;
            Ok(if report.failed_total() > 0 { 1 } else { 0 })
        }
        Command::Issue(cmd) => crate::work::issue(&engine, cmd, deps),
        Command::Run(cmd) => crate::work::run(&engine, cmd, deps),
        Command::Review(cmd) => crate::work::review(&engine, cmd, deps),
        Command::Pipeline(cmd) => crate::work::pipeline(&engine, cmd, deps),
        Command::Help => Ok(0),
    };
    engine.shutdown();
    result
}

fn sessions(engine: &Engine, deps: &mut Deps) -> Result<i32> {
    let filter = SessionFilter { parent: ParentFilter::TopLevel, archived: Some(false), ..Default::default() };
    let now = base::time::now_ms();
    let offset = base::time::utc_offset(now);
    for session in engine.sessions(&filter)? {
        let repo = session.workdir.as_ref().map(|w| format!("  [{}]", w.repo)).unwrap_or_default();
        let state = format!("{:?}", session.state).to_lowercase();
        let ago = base::time::ago(now, session.updated, offset);
        writeln!(deps.out, "{}  {state:<7}  {ago:>6}  {}{repo}", session.id, session.title)?;
    }
    Ok(0)
}

fn models(engine: &Engine, deps: &mut Deps) -> Result<i32> {
    let default = engine.settings().ok().and_then(|s| s.default_model);
    for entry in engine.models() {
        let provider = &entry.provider;
        let key = if provider.has_key { "" } else { " · no key" };
        writeln!(deps.out, "{} ({}{key}) {}", provider.id, provider.kind, provider.base_url)?;
        if let Some(error) = &entry.error {
            writeln!(deps.out, "  could not list models: {error}")?;
        }
        for model in &entry.models {
            let reference = ModelRef::new(provider.id.clone(), model.clone());
            let mark = if default.as_ref() == Some(&reference) { "  (default)" } else { "" };
            writeln!(deps.out, "  {reference}{mark}")?;
        }
    }
    Ok(0)
}

fn chat(engine: &Engine, chat: ChatArgs, deps: &mut Deps) -> Result<i32> {
    let events = engine.subscribe();
    let model = chat.model.as_deref().map(ModelRef::parse);
    let session = match &chat.session {
        Some(id) => {
            let id = SessionId::new(id.clone());
            let detail = engine.session_detail(&id)?.ok_or_else(|| anyhow!("there is no chat {id}"))?;
            if let Some(model) = &model {
                engine.set_session_model(&id, model)?;
            }
            detail.session
        }
        None => {
            let dir = chat.dir.clone().unwrap_or_else(|| deps.cwd.clone());
            let dir = if dir.is_absolute() { dir } else { deps.cwd.join(dir) };
            let workdir = WorkdirChoice { path: dir, mode: WorkMode::InPlace, base: None, branch: None };
            let new = NewSession {
                runnable: None,
                mode: Default::default(),
                model: model.clone(),
                workdir: Some(workdir),
                forced_skills: Vec::new(),
                issue: None,
            };
            engine.new_session(new)?
        }
    };
    let id = session.id.clone();
    let folder = session.workdir.as_ref().map(|w| w.path.display().to_string()).unwrap_or_default();
    let shown_model = model.unwrap_or(session.model);
    let shown_model = if shown_model.is_empty() { "no model".to_string() } else { shown_model.to_string() };
    writeln!(deps.err, "chat {id} · {shown_model} · {folder}")?;

    if let Some(message) = chat.message {
        let ok = turn(engine, &events, &id, message, chat.yes, deps)?;
        summary(engine, &id, deps)?;
        return Ok(if ok { 0 } else { 1 });
    }
    loop {
        write!(deps.err, "> ")?;
        deps.err.flush()?;
        let mut line = String::new();
        if deps.input.read_line(&mut line)? == 0 {
            writeln!(deps.err)?;
            break;
        }
        let line = line.trim();
        match line {
            "" => continue,
            "quit" | "exit" | "q" => break,
            _ => {
                turn(engine, &events, &id, line.to_string(), chat.yes, deps)?;
            }
        }
    }
    summary(engine, &id, deps)?;
    Ok(0)
}

/// Spend and tokens of the chat so far.
fn summary(engine: &Engine, id: &SessionId, deps: &mut Deps) -> Result<()> {
    if let Some(detail) = engine.session_detail(id)? {
        let s = &detail.session;
        writeln!(deps.err, "{} tokens · {}", base::fmt::tokens(s.usage.total()), base::fmt::cost(s.cost_usd))?;
    }
    Ok(())
}

/// Sends one message and prints the turn until the chat is idle again. Returns whether the
/// turn ended without failing.
fn turn(engine: &Engine, events: &Receiver<Event>, id: &SessionId, text: String, yes: bool, deps: &mut Deps) -> Result<bool> {
    signal::set_in_turn(true);
    let result = turn_inner(engine, events, id, text, yes, deps);
    signal::set_in_turn(false);
    result
}

fn turn_inner(engine: &Engine, events: &Receiver<Event>, id: &SessionId, text: String, yes: bool, deps: &mut Deps) -> Result<bool> {
    engine.send(id, UserMessage { text, images: Vec::new() })?;
    let mut printer = Printer::new();
    let mut ended = false;
    let ok = loop {
        if signal::take_pressed() {
            engine.interrupt(id);
            printer.break_line(&mut *deps.out);
            writeln!(deps.err, "^C stopping the turn")?;
        }
        let event = match events.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => {
                std::thread::sleep(POLL);
                continue;
            }
            Err(TryRecvError::Closed) => bail!("the engine stopped"),
        };
        match event {
            Event::Agent { session, event } => {
                let own = &session == id;
                if own && matches!(event, AgentEvent::TurnFinished { .. } | AgentEvent::Failed { .. }) {
                    ended = true;
                }
                printer.event(&mut *deps.out, own, &event);
            }
            Event::PermissionRequested { request } => {
                printer.break_line(&mut *deps.out);
                let decision = decide(&request, yes, deps)?;
                // The turn may have stopped meanwhile; then nobody waits for the answer.
                let _ = engine.answer_permission(&request.id, decision);
            }
            Event::SessionChanged { session } if &session.id == id && ended && session.state != SessionState::Running => {
                break session.state != SessionState::Failed;
            }
            Event::Notice { level, text } => {
                let label = match level {
                    NoticeLevel::Error => "error",
                    NoticeLevel::Warning => "warning",
                    NoticeLevel::Info | NoticeLevel::Success => "note",
                };
                printer.break_line(&mut *deps.out);
                writeln!(deps.err, "{label}: {text}")?;
            }
            _ => {}
        }
    };
    printer.break_line(&mut *deps.out);
    deps.out.flush()?;
    Ok(ok)
}

/// Asks on the terminal, or allows at once with `--yes`.
fn decide(request: &PermissionRequest, yes: bool, deps: &mut Deps) -> Result<PermissionDecision> {
    if yes {
        return Ok(PermissionDecision::AllowOnce);
    }
    write!(deps.err, "{}", render::question(request))?;
    deps.err.flush()?;
    let mut line = String::new();
    deps.input.read_line(&mut line)?;
    Ok(match line.trim().to_lowercase().as_str() {
        "y" | "yes" => PermissionDecision::AllowOnce,
        "a" | "always" => match &request.always_rule {
            Some(rule) => PermissionDecision::AllowAlways { rule: rule.clone() },
            None => PermissionDecision::AllowOnce,
        },
        _ => PermissionDecision::Reject { note: String::new() },
    })
}
