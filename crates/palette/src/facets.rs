//! The filter chips under the search field: what each one asks of the query and which count
//! of [`FacetCounts`] it shows.

use hub::api::{FacetCounts, SessionQuery, SessionState, StartedBy};

/// A chip of the filter row, left to right as in the mockup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Facet {
    All,
    Mine,
    StartedByAgents,
    Waiting,
    Running,
    Failed,
    HasPr,
    FromIssue,
}

impl Facet {
    pub const ALL: [Facet; 8] = [
        Facet::All,
        Facet::Mine,
        Facet::StartedByAgents,
        Facet::Waiting,
        Facet::Running,
        Facet::Failed,
        Facet::HasPr,
        Facet::FromIssue,
    ];

    /// The chip's text.
    pub fn label(self) -> &'static str {
        match self {
            Facet::All => "All",
            Facet::Mine => "Mine",
            Facet::StartedByAgents => "Started by agents",
            Facet::Waiting => "Waiting for you",
            Facet::Running => "Running",
            Facet::Failed => "Failed",
            Facet::HasPr => "Has PR",
            Facet::FromIssue => "From an issue",
        }
    }

    /// The probe name: `palette-chip-<this>`.
    pub fn slug(self) -> &'static str {
        match self {
            Facet::All => "all",
            Facet::Mine => "mine",
            Facet::StartedByAgents => "agents",
            Facet::Waiting => "waiting",
            Facet::Running => "running",
            Facet::Failed => "failed",
            Facet::HasPr => "pr",
            Facet::FromIssue => "issue",
        }
    }

    /// The count the chip shows, `None` when the mockup shows none and there is nothing to
    /// say. All, Mine and Started by agents always show theirs.
    pub fn count(self, facets: &FacetCounts) -> Option<u64> {
        let (count, always) = match self {
            Facet::All => (facets.all, true),
            Facet::Mine => (facets.mine, true),
            Facet::StartedByAgents => (facets.started_by_agents, true),
            Facet::Waiting => (facets.waiting, false),
            Facet::Running => (facets.running, false),
            Facet::Failed => (facets.failed, false),
            Facet::HasPr => (facets.has_pr, false),
            Facet::FromIssue => (facets.from_issue, false),
        };
        (always || count > 0).then_some(count)
    }

    /// The chip is on in `query`. `All` is on when no chip is.
    pub fn pressed(self, query: &SessionQuery) -> bool {
        match self {
            Facet::All => !Facet::ALL[1..].iter().any(|chip| chip.pressed(query)),
            Facet::Mine => query.started_by == Some(StartedBy::Me),
            Facet::StartedByAgents => query.started_by == Some(StartedBy::Agents),
            Facet::Waiting => query.state == Some(SessionState::Waiting),
            Facet::Running => query.state == Some(SessionState::Running),
            Facet::Failed => query.state == Some(SessionState::Failed),
            Facet::HasPr => query.has_pr,
            Facet::FromIssue => query.from_issue,
        }
    }

    /// Presses the chip: turns it on (replacing its sibling in the same family: Mine and
    /// Started by agents, or the three states), or off when it was on. `All` turns every chip off.
    pub fn toggle(self, query: &mut SessionQuery) {
        let on = self.pressed(query);
        match self {
            Facet::All => {
                query.started_by = None;
                query.state = None;
                query.has_pr = false;
                query.from_issue = false;
            }
            Facet::Mine => query.started_by = (!on).then_some(StartedBy::Me),
            Facet::StartedByAgents => query.started_by = (!on).then_some(StartedBy::Agents),
            Facet::Waiting => query.state = (!on).then_some(SessionState::Waiting),
            Facet::Running => query.state = (!on).then_some(SessionState::Running),
            Facet::Failed => query.state = (!on).then_some(SessionState::Failed),
            Facet::HasPr => query.has_pr = !on,
            Facet::FromIssue => query.from_issue = !on,
        }
    }
}

/// Whether any chip narrows the query.
pub(crate) fn filtered(query: &SessionQuery) -> bool {
    !Facet::All.pressed(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_is_on_until_a_chip_is_pressed_and_clears_them() {
        let mut query = SessionQuery::default();
        assert!(Facet::All.pressed(&query));
        Facet::Waiting.toggle(&mut query);
        Facet::HasPr.toggle(&mut query);
        assert!(!Facet::All.pressed(&query) && Facet::Waiting.pressed(&query) && Facet::HasPr.pressed(&query));
        Facet::All.toggle(&mut query);
        assert_eq!(query, SessionQuery::default());
    }

    #[test]
    fn chips_of_one_family_replace_each_other_and_toggle_off() {
        let mut query = SessionQuery::default();
        Facet::Mine.toggle(&mut query);
        Facet::StartedByAgents.toggle(&mut query);
        assert!(!Facet::Mine.pressed(&query) && Facet::StartedByAgents.pressed(&query));
        Facet::Running.toggle(&mut query);
        Facet::Failed.toggle(&mut query);
        assert_eq!(query.state, Some(SessionState::Failed));
        Facet::Failed.toggle(&mut query);
        assert_eq!(query.state, None);
        assert!(filtered(&query));
    }

    #[test]
    fn quiet_chips_show_a_count_only_when_there_is_one() {
        let facets = FacetCounts { all: 9, mine: 0, waiting: 0, failed: 2, ..FacetCounts::default() };
        assert_eq!(Facet::All.count(&facets), Some(9));
        assert_eq!(Facet::Mine.count(&facets), Some(0));
        assert_eq!(Facet::Waiting.count(&facets), None);
        assert_eq!(Facet::Failed.count(&facets), Some(2));
    }
}
