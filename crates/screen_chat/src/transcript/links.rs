//! Which permission question belongs on which tool row.
//!
//! The model keeps a question as its own block after the call it is about. The mockup draws it
//! on the call's row instead, so the view links the two: by the request's `call_id`, else (an
//! older engine, a fixture) by the newest earlier call of the same tool. A linked question
//! block draws nothing; its tool row shows the question while it is pending.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use hub::model::PermissionRequest;
use hub::transcript::{BlockKind, PermissionState};
use hub::{Block, Splice};

/// The links of one state of the transcript.
#[derive(Clone, Debug, Default)]
pub(crate) struct Links {
    /// Tool row index → the question pending on it.
    pending: HashMap<usize, Arc<PermissionRequest>>,
    /// Question rows drawn on their tool row.
    hidden: HashSet<usize>,
}

impl Links {
    /// Links every question of `blocks` to its tool row.
    pub fn compute(blocks: &[Arc<Block>]) -> Self {
        let mut links = Links::default();
        let mut by_call: HashMap<&str, usize> = HashMap::new();
        let mut by_name: HashMap<&str, usize> = HashMap::new();
        for (index, block) in blocks.iter().enumerate() {
            match &block.kind {
                BlockKind::Tool(tool) => {
                    by_call.insert(tool.id.as_str(), index);
                    by_name.insert(tool.name.as_str(), index);
                }
                BlockKind::Permission(question) => {
                    let request = &question.request;
                    let tool = match request.call_id.as_deref() {
                        Some(call) => by_call.get(call),
                        None => by_name.get(request.tool.as_str()),
                    };
                    if let Some(&tool) = tool {
                        links.hidden.insert(index);
                        if question.state == PermissionState::Pending {
                            links.pending.insert(tool, request.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        links
    }

    /// The question pending on tool row `index`.
    pub fn pending_on(&self, index: usize) -> Option<&Arc<PermissionRequest>> {
        self.pending.get(&index)
    }

    /// Question row `index` is drawn on its tool row.
    pub fn is_hidden(&self, index: usize) -> bool {
        self.hidden.contains(&index)
    }

    /// Tool rows that show a question.
    pub fn rows(&self) -> HashSet<usize> {
        self.pending.keys().copied().collect()
    }

    /// [`rows`](Self::rows) moved to the indices they have after `splice`; rows inside the
    /// replaced range are dropped (they are measured again anyway).
    pub fn moved_rows(&self, splice: &Splice) -> HashSet<usize> {
        self.pending
            .keys()
            .filter_map(|&index| {
                if index < splice.old.start {
                    Some(index)
                } else if index >= splice.old.end {
                    Some(index - splice.old.len() + splice.count)
                } else {
                    None
                }
            })
            .collect()
    }
}
