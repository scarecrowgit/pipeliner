//! Parsed documents keyed by their text, so a finished message is parsed once however often
//! its view is rebuilt, and a streaming message only re-parses its last block as it grows.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use super::parse::{Document, parse, parse_appended};

/// A bounded map from Markdown text to its parsed [`Document`].
pub struct ParseCache {
    entries: HashMap<u64, (Arc<str>, Arc<Document>)>,
    order: VecDeque<u64>,
    capacity: usize,
}

impl ParseCache {
    /// A cache holding at most `capacity` documents (the oldest go first).
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// Number of cached documents.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No document cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The document for `text`, parsed now unless cached. When `prev` is the document of a
    /// text that `text` extends (a message still streaming), only its last block is parsed
    /// again.
    pub fn get(&mut self, text: &str, prev: Option<(&str, &Document)>) -> Arc<Document> {
        let key = hash(text);
        if let Some((t, doc)) = self.entries.get(&key)
            && **t == *text
        {
            return doc.clone();
        }
        let doc = Arc::new(match prev {
            Some((prev_text, prev_doc)) if text.starts_with(prev_text) => {
                // The shorter text is a stage of the same stream: replace it, so a stream
                // does not push finished messages out.
                let old = hash(prev_text);
                if self
                    .entries
                    .get(&old)
                    .is_some_and(|(t, _)| **t == *prev_text)
                {
                    self.entries.remove(&old);
                    self.order.retain(|k| *k != old);
                }
                parse_appended(prev_doc, prev_text, text)
            }
            _ => parse(text),
        });
        if self.entries.len() >= self.capacity
            && let Some(old) = self.order.pop_front()
        {
            self.entries.remove(&old);
        }
        if self
            .entries
            .insert(key, (Arc::from(text), doc.clone()))
            .is_none()
        {
            self.order.push_back(key);
        }
        doc
    }
}

fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

thread_local! {
    static CACHE: RefCell<ParseCache> = RefCell::new(ParseCache::new(512));
}

/// [`ParseCache::get`] on the UI thread's shared cache.
pub fn parse_cached(text: &str, prev: Option<(&str, &Document)>) -> Arc<Document> {
    CACHE.with(|c| c.borrow_mut().get(text, prev))
}
