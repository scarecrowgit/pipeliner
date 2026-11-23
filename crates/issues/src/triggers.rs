//! Who wakes up when something happens on an issue.

use model::{Issue, IssueStatus, Runnable, RunnableRef};

#[cfg(test)]
mod tests;

/// Something that happened on an issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Happening {
    /// A runner was attached to an existing issue (the issue's `runner` is already set).
    Attached,
    /// The issue was created, with or without a runner.
    Created,
    /// A comment was posted. `mentions` are the runnables it names (see `mentions::find_mentions`).
    Comment { text: String, mentions: Vec<RunnableRef> },
    /// The issue was moved to a status by a person.
    MovedTo(IssueStatus),
}

/// How a woken runnable takes the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeMode {
    /// Starts a new run on the issue.
    Start,
    /// The attached runner carries on with this issue and gets the text as input.
    Continue,
}

/// A runnable that should be woken, and what to tell it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeIntent {
    pub target: RunnableRef,
    pub name: String,
    pub mode: WakeMode,
    /// For the activity feed: "comment passed as input", "moved to Todo".
    pub cause: String,
    /// What the run is handed: the comment, or the issue's title and body.
    pub input: String,
}

/// A runnable that should have been woken and cannot be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeRefusal {
    pub target: RunnableRef,
    pub name: String,
    /// "paused · takes no new work".
    pub reason: String,
}

/// The outcome of one happening: who to start and who could not be.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wakes {
    pub intents: Vec<WakeIntent>,
    pub refusals: Vec<WakeRefusal>,
}

struct Candidate<'a> {
    target: &'a RunnableRef,
    mode: WakeMode,
    cause: &'static str,
    input: String,
}

/// Decides who `happening` wakes on `issue`.
///
/// - The attached runner starts on `Attached` or `Created` when `on_assign` is set, on a move to
///   Todo when `on_todo` is set, and carries on with a comment when `on_comment` is set
///   ("comment passed as input").
/// - A mentioned runnable is woken by the mention. The attached one continues on this issue
///   (unless `on_mention` was switched off for it); any other starts a new run on the issue with
///   the comment as input.
/// - Nobody is woken twice by one happening, and the attached runner comes first.
/// - A paused or archived runnable (or one that no longer exists) is not woken; it appears in
///   `refusals` instead, once.
pub fn wake_intents(issue: &Issue, happening: &Happening, runnables: &[Runnable]) -> Wakes {
    let attached = issue.runner.as_ref();
    let triggers = &issue.triggers;
    let mut candidates: Vec<Candidate> = Vec::new();

    match happening {
        Happening::Attached | Happening::Created => {
            if let (Some(target), true) = (attached, triggers.on_assign) {
                let cause = if matches!(happening, Happening::Created) { "issue created" } else { "attached" };
                candidates.push(Candidate { target, mode: WakeMode::Start, cause, input: issue_input(issue) });
            }
        }
        Happening::MovedTo(IssueStatus::Todo) => {
            if let (Some(target), true) = (attached, triggers.on_todo) {
                candidates.push(Candidate {
                    target,
                    mode: WakeMode::Start,
                    cause: "moved to Todo",
                    input: issue_input(issue),
                });
            }
        }
        Happening::MovedTo(_) => {}
        Happening::Comment { text, mentions } => {
            if let Some(target) = attached {
                let mentioned = triggers.on_mention && mentions.contains(target);
                if mentioned || triggers.on_comment {
                    let cause = if mentioned { "@mention · comment passed as input" } else { "comment passed as input" };
                    candidates.push(Candidate { target, mode: WakeMode::Continue, cause, input: text.clone() });
                }
            }
            for target in mentions {
                if Some(target) != attached {
                    candidates.push(Candidate {
                        target,
                        mode: WakeMode::Start,
                        cause: "@mention · starts a new run",
                        input: text.clone(),
                    });
                }
            }
        }
    }

    let mut wakes = Wakes::default();
    let mut seen: Vec<&RunnableRef> = Vec::new();
    for c in candidates {
        if seen.contains(&c.target) {
            continue;
        }
        seen.push(c.target);
        match runnables.iter().find(|r| &r.reference == c.target) {
            Some(r) if r.archived => wakes.refusals.push(refusal(r, "archived")),
            Some(r) if r.paused => wakes.refusals.push(refusal(r, "paused · takes no new work")),
            Some(r) => wakes.intents.push(WakeIntent {
                target: r.reference.clone(),
                name: r.name.clone(),
                mode: c.mode,
                cause: c.cause.to_string(),
                input: c.input,
            }),
            None => wakes.refusals.push(WakeRefusal {
                target: c.target.clone(),
                name: c.target.id().to_string(),
                reason: "no longer exists".to_string(),
            }),
        }
    }
    wakes
}

fn refusal(r: &Runnable, reason: &str) -> WakeRefusal {
    WakeRefusal { target: r.reference.clone(), name: r.name.clone(), reason: reason.to_string() }
}

/// What a freshly started run is told about the issue: the title, then the body when it has one.
fn issue_input(issue: &Issue) -> String {
    let (title, body) = (issue.title.trim(), issue.body.trim());
    if body.is_empty() { title.to_string() } else { format!("{title}\n\n{body}") }
}
