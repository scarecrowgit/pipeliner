//! What the composer offers: models, `@` file mentions, `/` commands, and what a `/` command
//! turns into when it is sent.

use std::path::{Path, PathBuf};

use model::{Session, SessionId};

use crate::api::{ProviderModels, SlashCommand, UserMessage};
use crate::engine::Inner;

/// Files listed at most when completing an `@` mention.
const MENTION_SCAN: usize = 20_000;
/// Where a project keeps its commands.
const COMMANDS_DIR: &str = ".harness/commands";

/// What a sent message becomes.
pub(crate) enum Expanded {
    Message(UserMessage),
    /// `/compact`: summarise now instead of sending.
    Compact,
}

/// Expands `/name args` into the text of the project's command `name` (with `$ARGUMENTS`
/// replaced, or the arguments appended), and recognises `/compact`. Anything else is sent as
/// typed.
pub(crate) fn expand(session: &Session, message: UserMessage) -> Expanded {
    let text = message.text.trim();
    let Some(rest) = text.strip_prefix('/') else { return Expanded::Message(message) };
    let (name, args) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(n, a)| (n, a.trim()));
    if name == "compact" && args.is_empty() && message.images.is_empty() {
        return Expanded::Compact;
    }
    let Some(dir) = commands_dir(session) else { return Expanded::Message(message) };
    let Ok(body) = std::fs::read_to_string(dir.join(format!("{name}.md"))) else {
        return Expanded::Message(message);
    };
    let body = base::frontmatter::parse(&body).body.trim().to_string();
    let text = if body.contains("$ARGUMENTS") {
        body.replace("$ARGUMENTS", args)
    } else if args.is_empty() {
        body
    } else {
        format!("{body}\n\n{args}")
    };
    Expanded::Message(UserMessage { text, images: message.images })
}

fn commands_dir(session: &Session) -> Option<PathBuf> {
    let dir = session.workdir.as_ref()?.path.join(COMMANDS_DIR);
    dir.is_dir().then_some(dir)
}

/// Built-in commands, then the project's.
pub(crate) fn slash_commands(inner: &Inner, id: &SessionId) -> Vec<SlashCommand> {
    let command = |name: &str, description: &str, from_project| SlashCommand {
        name: name.to_string(),
        description: description.to_string(),
        from_project,
    };
    let mut out = vec![
        command("compact", "Summarise older turns now", false),
        command("clear", "Start over in a new chat", false),
    ];
    let dir = inner.store.session(id).ok().flatten().and_then(|s| commands_dir(&s));
    if let Some(dir) = dir {
        out.extend(project_commands(&dir).into_iter().map(|(name, description)| command(&name, &description, true)));
    }
    out
}

/// `(name, description)` of every `.md` file in `dir`, by name. The description is the
/// header's `description`, else the first line of the body.
fn project_commands(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_string_lossy().into_owned();
            if path.extension()? != "md" || name.starts_with('.') {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            let doc = base::frontmatter::parse(&text);
            let description = doc.str("description").map(str::to_string).unwrap_or_else(|| {
                let line = doc.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
                base::fmt::truncate_chars(line.trim_start_matches('#').trim(), 80)
            });
            Some((name, description))
        })
        .collect();
    out.sort();
    out
}

/// Files of the session's folder whose path contains `prefix`, those starting with it first.
pub(crate) fn mention_files(inner: &Inner, id: &SessionId, prefix: &str, limit: usize) -> Vec<String> {
    let Some(root) = inner.store.session(id).ok().flatten().and_then(|s| s.workdir).map(|w| w.path) else {
        return Vec::new();
    };
    let needle = prefix.to_lowercase();
    let files = tools::workspace::list_files(&root, MENTION_SCAN);
    let (mut first, mut rest): (Vec<String>, Vec<String>) = files
        .into_iter()
        .filter(|f| f.to_lowercase().contains(&needle))
        .partition(|f| f.to_lowercase().starts_with(&needle) || file_name(f).starts_with(&needle));
    first.append(&mut rest);
    first.truncate(limit);
    first
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_lowercase()
}

/// Every configured provider with its live model list, or its configured one and the error.
pub(crate) fn models(inner: &Inner) -> Vec<ProviderModels> {
    inner
        .llm
        .providers()
        .into_iter()
        .map(|provider| match inner.llm.list_models(&provider.id) {
            Ok(models) => ProviderModels { models, provider, error: None },
            Err(error) => ProviderModels { models: provider.models.clone(), provider, error: Some(error.to_string()) },
        })
        .collect()
}
