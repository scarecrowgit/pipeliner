//! What the runs of one tree share: the run [`crate::run`] started and every sub-agent below
//! it. Limits that hold for a single session do not hold for a tree, because every sub-agent
//! starts counting at zero; these do.

use std::sync::Mutex;

use model::SessionId;

use crate::sink::lock;

/// Sub-agents one tree may create in all, whatever each spawn policy allows.
pub(crate) const MAX_CREATED: u32 = 64;
/// Sub-agents of one tree running at the same moment; each has a thread.
pub(crate) const MAX_ALIVE: u32 = 16;

/// A budget a run works under: its own, or one of a session above it. A session's spend
/// includes the sessions below it, so every run of the tree checks every budget above it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cap {
    pub session: SessionId,
    pub usd: f64,
}

/// Why a tree takes no more sub-agents right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Full {
    Created,
    Alive,
}

/// The sub-agents of one tree, counted across all its sessions.
#[derive(Default)]
pub(crate) struct Tree {
    /// Created in all, and alive now.
    counts: Mutex<(u32, u32)>,
}

impl Tree {
    /// Takes the place of one more sub-agent; it is free again when the slot is dropped.
    pub fn reserve(&self) -> Result<Slot<'_>, Full> {
        let mut counts = lock(&self.counts);
        if counts.0 >= MAX_CREATED {
            return Err(Full::Created);
        }
        if counts.1 >= MAX_ALIVE {
            return Err(Full::Alive);
        }
        counts.0 += 1;
        counts.1 += 1;
        Ok(Slot { tree: self })
    }
}

/// The place of one live sub-agent in its tree.
pub(crate) struct Slot<'t> {
    tree: &'t Tree,
}

impl<'t> Slot<'t> {
    pub fn tree(&self) -> &'t Tree {
        self.tree
    }

    /// Gives the place back as if it had never been taken: the sub-agent was not created.
    pub fn cancel(self) {
        let mut counts = lock(&self.tree.counts);
        counts.0 = counts.0.saturating_sub(1);
        // Dropping `self` after this takes it out of the live count.
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut counts = lock(&self.tree.counts);
        counts.1 = counts.1.saturating_sub(1);
    }
}

/// Where a run stands in its tree: the shared counters and the budgets above it.
#[derive(Clone, Copy)]
pub(crate) struct Lineage<'t> {
    pub tree: &'t Tree,
    pub caps: &'t [Cap],
}

/// The budget of a sub-agent: what is left under every budget above it, and never more than
/// the spawn policy's cap. `None` when nothing limits it.
pub(crate) fn child_budget(remaining: Option<f64>, cap: Option<f64>) -> Option<f64> {
    match (remaining, cap) {
        (Some(left), Some(cap)) => Some(left.min(cap)),
        (left, cap) => left.or(cap),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_counts_created_and_alive_across_whoever_asks() {
        let tree = Tree::default();
        let mut slots: Vec<Slot<'_>> = (0..MAX_ALIVE).map(|_| tree.reserve().expect("room")).collect();
        assert_eq!(tree.reserve().err(), Some(Full::Alive));
        // One finishes: there is room for one more.
        slots.pop();
        slots.push(tree.reserve().expect("room"));
        // One that was never created counts neither way.
        slots.pop().expect("slot").cancel();
        assert_eq!(*lock(&tree.counts), (MAX_ALIVE, MAX_ALIVE - 1));
        drop(slots);
        for _ in MAX_ALIVE..MAX_CREATED {
            drop(tree.reserve().expect("room"));
        }
        assert_eq!(tree.reserve().err(), Some(Full::Created));
    }

    #[test]
    fn a_child_gets_the_smaller_of_what_is_left_and_the_cap() {
        assert_eq!(child_budget(Some(2.0), Some(0.5)), Some(0.5));
        assert_eq!(child_budget(Some(0.2), Some(0.5)), Some(0.2));
        assert_eq!(child_budget(Some(0.2), None), Some(0.2));
        assert_eq!(child_budget(None, Some(0.5)), Some(0.5));
        assert_eq!(child_budget(None, None), None);
    }
}
