//! What an import did: counts per kind and the list of things that could not be imported.

use std::fmt;

/// The kinds of things the import moves over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Session,
    Message,
    Issue,
    Run,
    Inbox,
    /// A render picture copied into the blob store.
    Image,
    /// The pinned and archived lists of the chat history.
    Flags,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Session => "session",
            Kind::Message => "message",
            Kind::Issue => "issue",
            Kind::Run => "run",
            Kind::Inbox => "inbox item",
            Kind::Image => "image",
            Kind::Flags => "chat flags",
        }
    }
}

/// How many things of one kind went each way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Written to the store (in a dry run: would be written).
    pub imported: usize,
    /// Already in the store from an earlier import.
    pub skipped: usize,
    /// Could not be imported; each has a [`Problem`].
    pub failed: usize,
}

/// One thing that could not be imported, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub kind: Kind,
    /// The file or id it concerns, as the old app named it.
    pub item: String,
    pub reason: String,
}

/// The outcome of [`crate::import_v1::import_v1`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    /// Nothing was written: the counts are what a real run would do.
    pub dry_run: bool,
    pub sessions: Tally,
    pub messages: Tally,
    pub issues: Tally,
    /// Pipeline runs of the old app, imported as history.
    pub runs: Tally,
    pub inbox: Tally,
    /// Render images copied to the blob store.
    pub images: Tally,
    pub problems: Vec<Problem>,
}

impl ImportReport {
    pub(super) fn tally_mut(&mut self, kind: Kind) -> Option<&mut Tally> {
        match kind {
            Kind::Session => Some(&mut self.sessions),
            Kind::Message => Some(&mut self.messages),
            Kind::Issue => Some(&mut self.issues),
            Kind::Run => Some(&mut self.runs),
            Kind::Inbox => Some(&mut self.inbox),
            Kind::Image => Some(&mut self.images),
            Kind::Flags => None,
        }
    }

    pub(super) fn imported(&mut self, kind: Kind, count: usize) {
        if let Some(tally) = self.tally_mut(kind) {
            tally.imported += count;
        }
    }

    pub(super) fn skipped(&mut self, kind: Kind, count: usize) {
        if let Some(tally) = self.tally_mut(kind) {
            tally.skipped += count;
        }
    }

    /// Records a failure: counted under its kind and listed with the reason.
    pub(super) fn fail(&mut self, kind: Kind, item: impl Into<String>, reason: impl Into<String>) {
        if let Some(tally) = self.tally_mut(kind) {
            tally.failed += 1;
        }
        self.problems.push(Problem {
            kind,
            item: item.into(),
            reason: reason.into(),
        });
    }

    fn tallies(&self) -> [(&'static str, Tally); 6] {
        [
            ("sessions", self.sessions),
            ("messages", self.messages),
            ("issues", self.issues),
            ("issue runs", self.runs),
            ("inbox items", self.inbox),
            ("images", self.images),
        ]
    }

    /// How many things were imported, over all kinds.
    pub fn imported_total(&self) -> usize {
        self.tallies().iter().map(|(_, t)| t.imported).sum()
    }

    /// How many things failed, over all kinds.
    pub fn failed_total(&self) -> usize {
        self.tallies().iter().map(|(_, t)| t.failed).sum::<usize>()
            + self.problems.iter().filter(|p| p.kind == Kind::Flags).count()
    }
}

impl fmt::Display for ImportReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.dry_run {
            writeln!(f, "Import from the old app (dry run, nothing was written)")?;
        } else {
            writeln!(f, "Import from the old app")?;
        }
        for (name, tally) in self.tallies() {
            let verb = if self.dry_run { "to import" } else { "imported" };
            writeln!(
                f,
                "  {name:<12} {} {verb}, {} already there, {} failed",
                tally.imported, tally.skipped, tally.failed
            )?;
        }
        if !self.problems.is_empty() {
            writeln!(f, "Problems ({}):", self.problems.len())?;
            for problem in &self.problems {
                writeln!(f, "  {} {}: {}", problem.kind.label(), problem.item, problem.reason)?;
            }
        }
        Ok(())
    }
}
