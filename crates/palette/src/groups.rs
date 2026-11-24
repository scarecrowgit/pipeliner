//! The list's sections: Pinned first, then Today, Yesterday, the last week, this and the
//! previous month, and everything older, each with its number of chats and their cost.
//!
//! Pure functions over search hits, so the grouping is tested without a window.

use std::collections::HashSet;

use base::fmt;
use base::time::{self, civil, day_start, from_civil, month_name};
use hub::api::{Millis, SessionHit};

const DAY: Millis = 86_400_000;

/// One section of the list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Group {
    /// The heading; also the key of its collapsed state.
    pub label: String,
    /// Indexes into the result's hits, in the order they are shown.
    pub hits: Vec<usize>,
    /// Sum of the chats' cost.
    pub cost: f64,
}

impl Group {
    /// The muted figure at the right of the heading: `12 · $0.44`.
    pub fn meta(&self) -> String {
        format!("{} · {}", fmt::count(self.hits.len() as u64), fmt::cost(self.cost))
    }
}

/// Where each section starts, from the clock and the time zone.
struct Edges {
    today: Millis,
    yesterday: Millis,
    week: Millis,
    month: Millis,
    previous_month: Millis,
    this_month_label: String,
    previous_month_label: String,
    older_label: String,
}

impl Edges {
    fn new(now: Millis, offset: i32) -> Self {
        let today = day_start(now, offset);
        let at = civil(now, offset);
        let (previous_year, previous) = if at.month == 1 { (at.year - 1, 12) } else { (at.year, at.month - 1) };
        let older = if previous == 1 { 12 } else { previous - 1 };
        Self {
            today,
            yesterday: today - DAY,
            week: today - 6 * DAY,
            month: from_civil(at.year, at.month, 1, 0, 0, 0, offset),
            previous_month: from_civil(previous_year, previous, 1, 0, 0, 0, offset),
            this_month_label: format!("{} {}", month_name(at.month), at.year),
            previous_month_label: format!("{} {}", month_name(previous), previous_year),
            older_label: format!("{} and older", month_name(older)),
        }
    }

    fn label(&self, updated: Millis) -> &str {
        if updated >= self.today {
            "Today"
        } else if updated >= self.yesterday {
            "Yesterday"
        } else if updated >= self.week {
            "Last 7 days"
        } else if updated >= self.month {
            &self.this_month_label
        } else if updated >= self.previous_month {
            &self.previous_month_label
        } else {
            &self.older_label
        }
    }
}

/// Splits `hits` (newest first) into sections. Pinned chats form the first section whatever
/// their age; the others follow in time order.
pub(crate) fn group(hits: &[SessionHit], now: Millis, offset: i32) -> Vec<Group> {
    let edges = Edges::new(now, offset);
    let mut pinned = Group { label: "Pinned".to_string(), hits: Vec::new(), cost: 0.0 };
    let mut groups: Vec<Group> = Vec::new();
    for (index, hit) in hits.iter().enumerate() {
        let target = if hit.session.pinned {
            &mut pinned
        } else {
            let label = edges.label(hit.session.updated);
            match groups.iter().position(|group| group.label == label) {
                Some(at) => &mut groups[at],
                None => {
                    groups.push(Group { label: label.to_string(), hits: Vec::new(), cost: 0.0 });
                    let last = groups.len() - 1;
                    &mut groups[last]
                }
            }
        };
        target.hits.push(index);
        target.cost += hit.session.cost_usd;
    }
    if !pinned.hits.is_empty() {
        groups.insert(0, pinned);
    }
    groups
}

/// The local UTC offset at `now`, for [`group`].
pub(crate) fn local_offset(now: Millis) -> i32 {
    time::utc_offset(now)
}

/// One line of the list: a section heading or a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    /// The heading of section `group`.
    Header(usize),
    /// Hit `hit`, the `row`-th selectable row.
    Row { hit: usize, row: usize },
}

/// What the list shows: sections and, unless a section is collapsed, its chats. Rows are what
/// the keyboard moves over.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Listing {
    pub groups: Vec<Group>,
    pub entries: Vec<Entry>,
    /// Hit index of each row.
    pub rows: Vec<usize>,
    /// Index into `entries` of each row, for scrolling it into view.
    pub row_entry: Vec<usize>,
}

impl Listing {
    pub fn new(groups: Vec<Group>, collapsed: &HashSet<String>) -> Self {
        let mut entries = Vec::new();
        let mut rows = Vec::new();
        let mut row_entry = Vec::new();
        for (at, group) in groups.iter().enumerate() {
            entries.push(Entry::Header(at));
            if collapsed.contains(&group.label) {
                continue;
            }
            for &hit in &group.hits {
                row_entry.push(entries.len());
                entries.push(Entry::Row { hit, row: rows.len() });
                rows.push(hit);
            }
        }
        Self { groups, entries, rows, row_entry }
    }

    /// The entry to scroll to for row `row`: its heading when it is the first of its section,
    /// so the heading comes into view with it.
    pub fn scroll_target(&self, row: usize) -> Option<usize> {
        let entry = *self.row_entry.get(row)?;
        match entry.checked_sub(1).and_then(|before| self.entries.get(before)) {
            Some(Entry::Header(_)) => Some(entry - 1),
            _ => Some(entry),
        }
    }
}

#[cfg(test)]
mod tests {
    use hub::api::{Session, SessionId};

    use super::*;

    const NOW: Millis = 1_795_000_000_000;
    const HOUR: Millis = 3_600_000;

    fn hit(id: &str, updated: Millis, pinned: bool, cost: f64) -> SessionHit {
        let mut session = hub::fixtures::World::acme(NOW).sessions.remove(0);
        session.id = SessionId::new(id);
        session.updated = updated;
        session.pinned = pinned;
        session.cost_usd = cost;
        hit_of(session)
    }

    fn hit_of(session: Session) -> SessionHit {
        SessionHit { session, children: 0, title_highlights: Vec::new(), snippet: None }
    }

    fn labels(groups: &[Group]) -> Vec<&str> {
        groups.iter().map(|group| group.label.as_str()).collect()
    }

    #[test]
    fn pinned_chats_lead_and_the_rest_follow_in_time_order() {
        let hits = [
            hit("a", NOW - HOUR, false, 0.1),
            hit("b", NOW - 3 * DAY, true, 0.2),
            hit("c", NOW - 40 * DAY, false, 0.3),
            hit("d", NOW - 100 * DAY, false, 0.4),
        ];
        let groups = group(&hits, NOW, 0);
        let names = labels(&groups);
        assert_eq!(names[0], "Pinned");
        assert!(names[1] == "Today" || names[1] == "Yesterday", "{names:?}");
        assert!(names.last().is_some_and(|label| label.ends_with("and older")), "{names:?}");
        assert_eq!(groups[0].hits, [1]);
        let total: usize = groups.iter().map(|group| group.hits.len()).sum();
        assert_eq!(total, 4, "every hit is in exactly one section");
    }

    #[test]
    fn sections_sum_their_cost_and_say_so() {
        let hits = [hit("a", NOW - HOUR, false, 0.25), hit("b", NOW - HOUR, false, 0.19)];
        let groups = group(&hits, NOW, 0);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].meta(), "2 · $0.44");
    }

    #[test]
    fn a_collapsed_section_keeps_its_heading_and_loses_its_rows() {
        let hits = [hit("a", NOW - HOUR, true, 0.0), hit("b", NOW - HOUR, false, 0.0), hit("c", NOW - 2 * HOUR, false, 0.0)];
        let groups = group(&hits, NOW, 0);
        let open = Listing::new(groups.clone(), &HashSet::new());
        assert_eq!(open.rows, [0, 1, 2]);
        assert_eq!(open.entries.len(), 5);
        assert_eq!(open.scroll_target(0), Some(0), "the first row reveals its heading");
        assert_eq!(open.scroll_target(2), Some(4), "a later row scrolls to itself");

        let collapsed = Listing::new(groups, &HashSet::from(["Pinned".to_string()]));
        assert_eq!(collapsed.rows, [1, 2]);
        assert_eq!(collapsed.entries[0], Entry::Header(0));
        assert_eq!(collapsed.entries.len(), 4);
    }
}
