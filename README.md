# Harness

Harness is a desktop app for working with coding agents. It is written in Rust (2024 edition)
with GPUI 0.2.2. This branch (`recode`) is a rewrite of the first app, built to the `screens-v2`
designs.

You talk to an agent in a chat, or you file an issue and let a pipeline of agents work on it in
git worktrees. Work that needs you lands in an Inbox: a review to approve, a question to answer,
a run that stopped. Agents, orchestrators and pipelines are files you can edit, by hand or in the
app. A command-line client, `harness-cli`, does the same things without a window.

Screens:

| Screen | What it is for |
| --- | --- |
| Home | What needs you, what is working now, and spend, runs and agents over a period. |
| Chats | The list of chats, a chat with its context panel, and the new-chat page (agent, folder, mode). |
| Inbox | Reviews (variants side by side, diffs, renders), agent questions, reports, pull requests. |
| Issues | A board and a list per project; one issue with its properties, runs, activity and comments; autopilots. |
| Agents | The library of agents, orchestrators and pipelines: create, duplicate, import, export, archive, delete. |
| Pipeline editor | A canvas of nodes and edges, an inspector, and a dock with problems, test runs and deploys. |

Keys: Ctrl-K (Cmd-K on macOS) opens the chat search, Ctrl-N starts a new chat, Ctrl-\ collapses
the sidebar.

## Build and run

Cargo is available inside the Nix shell. Close stdin, as the commands below do.

```bash
nix-shell shell.nix --run 'cargo build --release --bin harness --bin harness-cli' </dev/null
nix-shell shell.nix --run 'cargo run --release --bin harness' </dev/null
```

The shell provides the libraries GPUI loads at run time (Wayland, xkbcommon, Vulkan, fontconfig).
Outside the shell the binary will not start. To get a binary you can start from anywhere:

```bash
scripts/build-desktop.sh              # build, write dist/, install the menu entry "Harness"
scripts/build-desktop.sh --no-menu    # build only
scripts/build-desktop.sh --no-build   # install what target/release already holds
```

This writes `dist/harness` (a launcher that sets `LD_LIBRARY_PATH`), `dist/harness-bin` and
`dist/harness-cli`. The app from before the rewrite is kept as `dist/harness-v1`; its source is
the tag `v1-legacy`.

To look at the UI without a model or any data, start it on the fixture backend:

```bash
harness --fixture <name>
```

The names are `home`, `sidebar`, `chat-session`, `chat-streaming`, `chat-history`, `new-chat`,
`inbox-review`, `issue-board`, `issue-comments`, `agents`, `pipeline-editor` and `empty`. A fixture
world is in memory. It never touches your config or data.

## First steps

**1. Configure a provider.** Harness reads `~/.config/harness/config.toml`. The format is the
old app's, so an existing file keeps working.

```toml
default_model = "anthropic/claude-sonnet-5-5"     # "<provider id>/<model id>"

[[providers]]
id = "anthropic"
kind = "anthropic"                    # or "openai" (any OpenAI-compatible API)
api_key_env = "ANTHROPIC_API_KEY"     # read the key from this variable
models = ["claude-sonnet-5-5"]        # optional
api_key = "sk-…"                      # or write the key in the file
```

Without a file, providers are found from the environment: `ANTHROPIC_API_KEY` gives `anthropic`,
`OPENAI_API_KEY` (and `OPENAI_BASE_URL`) gives `openai`, `OPENROUTER_API_KEY` gives `openrouter`,
and a running Ollama (`OLLAMA_HOST`, or a server on `localhost:11434`) gives `ollama`.
`ANTHROPIC_BASE_URL` is ignored on purpose. Check the result with `harness-cli models`.

There is no settings screen for providers yet. Edit the file.

**2. Chat on a repository.** Press Ctrl-N, pick an agent and a folder, and write. The default
agent is Build; Plan reads and proposes but edits nothing. A chat works in its own git worktree
and branch by default, so other work in the repository is not touched. The folder chip also offers
editing in place and read-only. Tool calls that write or run commands ask you first.

**3. Create an issue and run a pipeline.**

1. Make a pipeline: Agents, New, start from a template (for example "Agent → pull request" or
   "Agent ×3 → review → pull request"). In the editor, check that its workdir points at your
   repository, fix what the Problems list shows, and run a test run.
2. Deploy it. Issues start the deployed version.
3. Issues, New issue, attach the pipeline, and start it.
4. When the run reaches a review, it appears in the Inbox. Ship a variant, skip, or send back
   with a note.

Issues live in a project. Create one from the project switcher at the top of the sidebar
("New project": a name, the prefix of its issue keys, and the folders of its repositories).
The import of the old app brings its project along, and the first `harness-cli issue new`
creates a project called "Default" if none exists.

## The CLI

`harness-cli` opens the same data as the app, so a run started in one shows in the other.

```bash
harness-cli help
harness-cli models                      # providers and models
harness-cli sessions                    # chats, most recent first (chat --session <id> continues one)
```

A chat (one turn with a message; without one, a turn per line until `quit`):

```bash
harness-cli chat --dir ~/src/app "why does the login test fail?"
harness-cli chat --dir ~/src/app --model anthropic/claude-sonnet-5-5 --yes "fix it"
```

`--yes` allows every tool call without asking. `chat` works in place in `--dir` (default: the
current folder). Ctrl-C stops the running turn.

An issue, its run and its review:

```bash
harness-cli issue new "Fix login redirect" --body "After login the user lands on /." --pipeline agent-pr
harness-cli issue start HAR-1           # runs the attached pipeline, following it
harness-cli run list
harness-cli run log <run> -f            # follow the log
harness-cli review list
harness-cli review show <review>
harness-cli review ship <review> B      # ship variant B (or 2)
harness-cli review send-back <review> "keep the old API" --from variant:2
```

A command that starts or continues a run follows it until it needs you or ends. If the CLI exits
first, the run is interrupted; `run resume <run>` continues it. Other commands: `run cancel`,
`run answer <run> <text> [--node <id>]`, `review skip`, `issue list|show|comment`.

A test run of a pipeline:

```bash
harness-cli pipeline problems my-pipeline      # what blocks a deploy
harness-cli run start my-pipeline "Add a --json flag" --test
harness-cli run discard <run>                  # remove its throwaway worktrees and branches
harness-cli pipeline deploy my-pipeline --note "first version"
harness-cli run start my-pipeline "Add a --json flag"    # the deployed version
```

## Pipelines in brief

A pipeline is a graph in a TOML file. An item (an issue, a manual input or an autopilot's prompt)
enters at a source and follows `route` edges. A node starts when every route edge into it has
delivered the item or is dead; it is skipped when all of them are dead.

| Node | What it does |
| --- | --- |
| Source | Where the item comes from: an issue, manual input, or a schedule. |
| Router | Rules tried top to bottom, by a condition on the item or a yes/no question to a model with read-only tools. Each rule is an out-port; `else` is taken when none match. |
| Agent | A library agent in a worktree. `variants = N` runs N attempts, each in its own worktree and branch. |
| Orchestrator | An agent that leads a team and may create sub-agents. |
| Sub-agent | A template an agent may create with its `spawn_agent` tool. Attached by a `spawn` edge. |
| File | Files given to an agent as context (read), or an artifact it writes (write). Attached by a `context` edge. |
| Review | Waits for a person. Ship picks one variant and continues along `approved`. Skip ends the run. Send back restarts an earlier node with a note. |
| Action | `command` (a shell command; a tests command shows pass counts), `render` (changed HTML files to before and after images), `merge`, `pull_request` (commit, push, `gh pr create`), `report` (an Inbox item), `update_issue`. |

**Reviews.** After agents with several variants, a review shows what each variant left behind:
changed files, test results, renders. After Send back, the node named by the review's
`send_back_to` runs again with your note. You choose to keep one variant's work, give the note to
all variants, or start over on new branches. A pipeline limits this to `max_loops` rounds
(default 5), then the run stops and asks. A pipeline also has `max_parallel` and an optional
`budget_usd`.

**Test runs.** A test run uses the draft, not the deployed version. Agents are real and work in
throwaway worktrees. Merges, pushes, pull requests, reports and issue updates are only logged, so
nothing leaves the machine. A normal run uses a deployed version and stays on it while the draft
changes.

## Where things live

| What | Where |
| --- | --- |
| Config and providers | `~/.config/harness/config.toml` (`$HARNESS_CONFIG` names another file) |
| Your agent library | `~/.config/harness/library/{agents,orchestrators,pipelines}/` |
| Your skills | `~/.config/harness/skills/<name>/SKILL.md` |
| Project definitions, checked in | `<repo>/.harness/{agents,orchestrators,pipelines,skills}/` |
| Project slash commands | `<repo>/.harness/commands/` |
| Database (chats, issues, runs, inbox, settings) | `~/.local/share/harness/v2/harness.db` |
| Large payloads (attachments, tool output) | `~/.local/share/harness/v2/blobs/` |
| Logs | `~/.local/share/harness/v2/log/` |
| Worktrees of chats and runs | `~/.harness/worktrees/` |
| Old app's data (read only, for import) | `~/.local/share/harness/` and `~/.config/harness/pipelines/` |

`$HARNESS_DATA` replaces `~/.local/share/harness`; the new app writes only under its `v2/`
folder. `$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` are honoured when they are absolute paths.

Agents and orchestrators are Markdown files with a Claude Code style header (`name`,
`description`, `tools`, `model`) and a `harness:` block for the rest, so one file opens in both
tools. Pipelines are TOML. A file's id is its name without the extension. Later locations override
earlier ones with the same id: built-ins (Build and Plan), Claude Code agents (`~/.claude/agents`,
`<repo>/.claude/agents`, read only), your library, then the project.

MCP servers (stdio only) are read, in this order, from `[[mcp_servers]]` in `config.toml`
(`name`, `command`, `args`, `env`, `cwd`, `enabled`, `auto_approve`), `<workspace>/.mcp.json`,
and with `import_claude_code = true` from `~/.claude.json`.

## Importing from the old app

The old app kept chats, issues, pipeline runs and the Inbox as files in `~/.local/share/harness/`.
Import them once:

```bash
harness-cli import --dry-run     # parse everything, write nothing, print the report
harness-cli import
```

The import never writes to, renames or deletes the old files, so the old app keeps working. It
can be repeated: what was imported is recognised and skipped, and only new items are added (a chat
already imported is not updated with later messages). A bad file is counted and named in the
report; it does not stop the rest. Imported runs are history only.

## Safety

Read this before you give a pipeline a repository.

- A pipeline agent runs with nobody to ask. Every tool call its access level grants is allowed.
- Its shell commands and file tools start in the worktree, but the worktree is a working
  directory, not a sandbox. A command can read and write anywhere your user can.
- Set the access level on each agent. `ReadOnly` gives read tools only, `ReadAndRun` adds bash,
  `Full` (the default) adds editing and writing. Use `ReadOnly` or `ReadAndRun` where it matters.
- Chats ask before writes and commands, unless you use `--yes` in the CLI or allow a tool always.
- Provider keys are replaced by `[redacted]` in run logs, summaries, errors, commit messages, pull
  requests and Inbox items. Exports of a transcript redact secrets.
- Pull requests are made with the `gh` CLI, so they use your own GitHub login.

## Not available yet

These are drawn in the UI, marked "Coming soon" or disabled, and do nothing.

| Area | What |
| --- | --- |
| Runtimes | Cloud sandboxes and remote machines. Only the local runtime works. |
| Teams | Invite people, Switch account. |
| Sharing | Share a chat with "People in your workspace" or "Anyone with the link". |
| External harnesses | Claude Code and Codex CLI as the agent of a chat. |
| Sources | Jira and Sentry as a pipeline source. A run that reaches one fails with a message. |
| Autopilots | New autopilot from the popover, Run history, event triggers. Schedule autopilots fire while the app is open; one missed by less than a day fires once on start. |
| Pipeline editor | Nested pipeline nodes (the validator reports an error), staged rollout ("1 in 10 new issues for 24 hours"), Compare canvases, Replay last 100. |
| Chat | LSP diagnostics in the context panel. |

Also missing: a settings screen for providers, and an import button in the app (use the CLI).

## Development

The workspace has four layers and dependencies point down only: domain crates (`base`, `model`,
`store`, `llm`, `mcp`, `tools`, `vcs`, `catalog`, `agent`, `issues`, `pipeline`, `engine`) with no
GPUI; UI kit (`ui`, `ui_rich`); the bridge (`hub`); and views (`shell`, `palette`, `screen_*`),
which never use another screen or domain logic. `crates/devtools/tests/layers.rs` enforces this.
[AGENTS.md](AGENTS.md) has the full rules, the conventions and the GPUI quirks seen here.

```bash
nix-shell shell.nix --run 'cargo test -p <crate>' </dev/null
nix-shell shell.nix --run 'cargo test --workspace' </dev/null
nix-shell shell.nix --run 'cargo clippy --workspace --all-targets' </dev/null
```

Domain crates build faster in their own target directory:
`CARGO_TARGET_DIR=target/domain cargo test -p <crate>`. A change is done when the build is clean,
tests pass and clippy has no warnings. Tests use temp directories and scripted fakes. They never
touch your config or data and never call a real model.

`harness-dev` (crate `devtools`) drives a running window for scripted checks and screenshots:
`harness-dev run <binary> --script <file> --shot <png> -- --fixture home`, plus `mockup`, `shot`,
`compare` and `drive`. Run `harness-dev` without arguments for the usage text.

The mockups are the visual source of truth. They live outside the repository, in
`~/Pictures/designs/pipeliner-mockups/screens-v2/`.

Licence: MIT.
