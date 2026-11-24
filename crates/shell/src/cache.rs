//! The views the shell keeps alive between navigations (the rule is on [`Keep`]).

use hub::Route;

use crate::screen::{Keep, ScreenKind};

/// What a built view is kept under: its kind, plus the whole route for [`Keep::PerRoute`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CacheKey {
    pub kind: ScreenKind,
    pub route: Option<Route>,
}

impl CacheKey {
    /// The key `route` is shown under when its screen keeps views as `keep` says.
    pub fn new(route: &Route, keep: Keep) -> Self {
        let kind = ScreenKind::of(route);
        match keep {
            Keep::One => Self { kind, route: None },
            Keep::PerRoute(_) => Self { kind, route: Some(route.clone()) },
        }
    }
}

/// Built views, least recently shown first.
pub(crate) struct ScreenCache<T> {
    entries: Vec<(CacheKey, T)>,
}

impl<T> Default for ScreenCache<T> {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

impl<T> ScreenCache<T> {
    /// The view kept under `key`, marked as the most recently shown.
    pub fn touch(&mut self, key: &CacheKey) -> Option<&T> {
        let ix = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(ix);
        self.entries.push(entry);
        self.entries.last().map(|(_, value)| value)
    }

    /// Keep `value` under `key` as the most recently shown, then drop the oldest views of the
    /// same kind beyond what `keep` allows.
    pub fn insert(&mut self, key: CacheKey, value: T, keep: Keep) {
        self.entries.retain(|(k, _)| k != &key);
        let kind = key.kind;
        self.entries.push((key, value));
        let allowed = match keep {
            Keep::One => 1,
            Keep::PerRoute(n) => n.max(1),
        };
        let mut of_kind = self.entries.iter().filter(|(k, _)| k.kind == kind).count();
        while of_kind > allowed {
            match self.entries.iter().position(|(k, _)| k.kind == kind) {
                Some(ix) => {
                    self.entries.remove(ix);
                    of_kind -= 1;
                }
                None => break,
            }
        }
    }

    /// How many views are kept.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hub::api::SessionId;

    fn chat(id: &str) -> Route {
        Route::Chat { session: SessionId::new(id) }
    }

    #[test]
    fn one_view_serves_every_route_of_a_kind() {
        let a = CacheKey::new(&Route::Issues { issue: None }, Keep::One);
        let b = CacheKey::new(&Route::Issues { issue: Some(hub::api::IssueKey::new("PAY-1")) }, Keep::One);
        assert_eq!(a, b);
    }

    #[test]
    fn per_route_keeps_the_most_recent_and_drops_the_oldest() {
        let mut cache = ScreenCache::default();
        let keep = Keep::PerRoute(2);
        for id in ["a", "b"] {
            cache.insert(CacheKey::new(&chat(id), keep), id, keep);
        }
        // Showing "a" again makes "b" the oldest.
        assert_eq!(cache.touch(&CacheKey::new(&chat("a"), keep)), Some(&"a"));
        cache.insert(CacheKey::new(&chat("c"), keep), "c", keep);
        assert!(cache.touch(&CacheKey::new(&chat("b"), keep)).is_none());
        assert!(cache.touch(&CacheKey::new(&chat("a"), keep)).is_some());
        assert!(cache.touch(&CacheKey::new(&chat("c"), keep)).is_some());
        // Other kinds are not counted against the limit.
        cache.insert(CacheKey::new(&Route::Home, Keep::One), "home", Keep::One);
        assert_eq!(cache.len(), 3);
    }
}
