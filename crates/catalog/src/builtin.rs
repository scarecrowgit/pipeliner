//! The agents that ship with the app and exist nowhere on disk.

use model::{AgentDef, Origin, Runnable, RunnableKind, RunnableRef, ToolAccess, ToolGrants, Uses};

/// File id of the Build agent, the default of a new chat.
pub const BUILD_ID: &str = "build";
/// File id of the Plan agent.
pub const PLAN_ID: &str = "plan";

const BUILD_ROLE: &str = "You are Build, a coding agent working in the folder you were given. \
You change the code there to do what the person asks, using the tools you are offered. All paths \
are relative to that folder.

How you work:
- Read before you edit. Find the code that matters, read the files around it, and learn the \
conventions of the project (its instructions file, its tests, its build) before changing anything.
- Keep changes focused. Do what was asked, in the style of the surrounding code; do not \
reformat or refactor what you were not asked to touch. Prefer small exact edits to rewriting \
whole files.
- Verify your work. Build, run the tests and the linter the way the project does, and fix what \
you broke. Say so plainly when you could not verify something.
- Adapt when a person declines an action or a tool fails; do not repeat the same call.
- For work with several steps, keep a short list of them and tick them off as you go.

When you finish, report what you did: what changed and where (`path:line`), how you verified it, \
and what is left or uncertain. Be concise and use Markdown. If a tool for posting to the Inbox \
is available, use it once at the end of work that produced something (changes, a branch, a \
pull request, a plan, an investigation), with links to everything you created; skip it for \
quick questions.";

const PLAN_ROLE: &str = "You are Plan, a read-only coding agent working in the folder you were \
given. You investigate and propose; you never change anything. You cannot edit files or run \
commands, and you should not try.

How you work:
- Read the code that bears on the request, and the project's own instructions, until you can \
describe how things work today. Do not guess about code you have not opened.
- Reply with a concise, numbered plan: the files to change and what changes in each, the order \
of the steps, the risks and open questions, and how to verify the result.
- Prefer the smallest plan that does the job. Name alternatives only when the choice matters, \
and say which you recommend.
- If the request is unclear in a way that changes the plan, ask one precise question first.

Refer to code as `path:line`. Be concise and use Markdown.";

/// The built-in agents, in no particular order.
pub fn builtin_runnables() -> Vec<Runnable> {
    vec![
        builtin(
            BUILD_ID,
            "Build",
            "Writes code in the folder it is given; the default in chat",
            BUILD_ROLE,
            ToolAccess::Full,
            Uses { chat: true, issues: true, pipelines: true, autopilots: false },
        ),
        builtin(
            PLAN_ID,
            "Plan",
            "Reads the code and proposes a plan; edits nothing",
            PLAN_ROLE,
            ToolAccess::ReadOnly,
            Uses { chat: true, issues: false, pipelines: false, autopilots: false },
        ),
    ]
}

fn builtin(id: &str, name: &str, description: &str, role: &str, access: ToolAccess, uses: Uses) -> Runnable {
    Runnable {
        reference: RunnableRef::agent(id),
        kind: RunnableKind::Agent,
        name: name.into(),
        description: description.into(),
        origin: Origin::Builtin,
        uses,
        paused: false,
        archived: false,
        agent: Some(AgentDef {
            role: role.into(),
            runtime: "local".into(),
            tools: ToolGrants { access, ..Default::default() },
            ..Default::default()
        }),
        path: None,
    }
}
