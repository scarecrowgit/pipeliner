//! A chat transcript as a flat, append-mostly list of blocks a view renders with GPUI's
//! `list()`. Blocks are built from stored messages (history) and from the live
//! [`AgentEvent`](crate::api::AgentEvent) stream; every change is reported as a [`Splice`] so the
//! view re-measures only what changed.

mod history;
mod live;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use crate::api::{
    Attachment, Millis, ModelRef, PermissionRequest, RequestId, SessionId, TodoItem, ToolClass, ToolDisplay,
};

pub(crate) use live::LiveEffect;

/// Stable identity of a block, for element ids. Never reused within a transcript.
pub type BlockId = u64;

/// One row of the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
}

/// What a transcript row shows.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    /// What the person sent.
    User(UserBlock),
    /// The agent's text; `streaming` while deltas still arrive.
    Assistant(TextBlock),
    Thinking(TextBlock),
    /// One tool call moving from started to ready to finished.
    Tool(ToolBlock),
    /// A sub-agent the agent started.
    Subagent(SubagentBlock),
    /// The agent's todo list as of that point.
    Todos(Vec<TodoItem>),
    /// A tool call waiting for a person.
    Permission(PermissionBlock),
    /// Older turns were summarised above this line.
    Compaction(CompactionBlock),
    /// A line for the person that is not part of the conversation.
    Notice(NoticeBlock),
    /// The turn failed.
    Failure(String),
}

/// A message the person sent.
#[derive(Clone, Debug, PartialEq)]
pub struct UserBlock {
    pub seq: u64,
    pub at: Millis,
    pub text: String,
    pub images: Vec<Attachment>,
}

/// Text of the agent: its answer or its reasoning.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// Set once the message is stored.
    pub seq: Option<u64>,
    pub at: Option<Millis>,
    pub model: Option<ModelRef>,
    pub text: String,
    pub streaming: bool,
}

/// Where a tool call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    /// The model began the call; its input is still streaming.
    Started,
    /// The input is complete; it runs now or waits for a person.
    Ready,
    Finished { is_error: bool },
}

/// One tool call as a row.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolBlock {
    pub id: String,
    pub name: String,
    pub class: Option<ToolClass>,
    /// `None` while the input streams.
    pub display: Option<ToolDisplay>,
    pub state: ToolState,
}

/// Where a sub-agent is.
#[derive(Clone, Debug, PartialEq)]
pub enum SubagentState {
    Running,
    Finished { ok: bool, summary: String, tool_calls: u32, secs: u64, cost_usd: f64 },
}

/// A sub-agent row.
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentBlock {
    /// The sub-agent's own session, when known.
    pub session: Option<SessionId>,
    /// The tool call that started it, when it came from history.
    pub call: Option<String>,
    /// "@explore".
    pub name: String,
    pub task: String,
    pub depth: u32,
    pub state: SubagentState,
}

/// Whether a permission request was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    Pending,
    Resolved,
}

/// A permission request as a row.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionBlock {
    pub request: Arc<PermissionRequest>,
    pub state: PermissionState,
}

/// The divider where older turns were summarised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactionBlock {
    /// Messages replaced by the summary; 0 while it runs.
    pub summarised: u32,
    pub running: bool,
}

/// What a notice row is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    /// The model call is retried.
    Retry,
    /// The person stopped the turn.
    Interrupted,
    /// The agent opened a pull request; the text is its URL.
    PullRequest,
}

/// A line for the person that is not part of the conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct NoticeBlock {
    pub kind: NoticeKind,
    pub text: String,
}

/// A change to the block list in `ListState::splice` terms: the blocks in `old` (indices before
/// the change) were replaced by `count` blocks starting at `old.start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub old: Range<usize>,
    pub count: usize,
}

/// The blocks plus what is needed to keep building them.
#[derive(Debug, Default)]
pub struct Transcript {
    blocks: Vec<Arc<Block>>,
    next_id: BlockId,
    /// First index touched since the last `take_splice`, and the length at that moment.
    dirty: Option<(usize, usize)>,
    /// Tool results whose call is not loaded (yet): id → (output, is_error).
    orphans: HashMap<String, (String, bool)>,
    /// The highest stored message seq applied.
    newest_seq: Option<u64>,
    /// Compacted messages seen since the last divider.
    compacted_run: u32,
    /// Whether the oldest loaded message was compacted.
    oldest_compacted: Option<bool>,
}

impl Transcript {
    /// Every row, oldest first.
    pub fn blocks(&self) -> &[Arc<Block>] {
        &self.blocks
    }

    /// How many rows there are.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// There is no row yet.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// One row by index.
    pub fn get(&self, index: usize) -> Option<&Arc<Block>> {
        self.blocks.get(index)
    }

    /// The highest stored message seq in the transcript.
    pub fn newest_seq(&self) -> Option<u64> {
        self.newest_seq
    }

    /// Records that block `index` (or the end, for a push) is about to change.
    fn touch(&mut self, index: usize) {
        self.dirty = Some(match self.dirty {
            None => (index, self.blocks.len()),
            Some((from, base)) => (from.min(index), base),
        });
    }

    fn push(&mut self, kind: BlockKind) {
        self.touch(self.blocks.len());
        let id = self.next_id;
        self.next_id += 1;
        self.blocks.push(Arc::new(Block { id, kind }));
    }

    fn edit(&mut self, index: usize, f: impl FnOnce(&mut BlockKind)) {
        if index < self.blocks.len() {
            self.touch(index);
            f(&mut Arc::make_mut(&mut self.blocks[index]).kind);
        }
    }

    /// The last block matching `pred`, searching from the end.
    fn rfind(&self, pred: impl Fn(&BlockKind) -> bool) -> Option<usize> {
        self.blocks.iter().rposition(|block| pred(&block.kind))
    }

    /// Index of the first block after the last user message: where the current turn starts.
    fn turn_start(&self) -> usize {
        self.rfind(|kind| matches!(kind, BlockKind::User(_))).map_or(0, |i| i + 1)
    }

    fn tool_index(&self, id: &str) -> Option<usize> {
        self.rfind(|kind| matches!(kind, BlockKind::Tool(tool) if tool.id == id))
    }

    /// What changed since the last call, as one splice.
    pub(crate) fn take_splice(&mut self) -> Option<Splice> {
        let (from, base) = self.dirty.take()?;
        Some(Splice { old: from..base, count: self.blocks.len() - from })
    }

    /// Adds a permission request as a pending block, unless it is already there.
    pub(crate) fn permission(&mut self, request: Arc<PermissionRequest>) -> bool {
        let exists = self.rfind(|kind| matches!(kind, BlockKind::Permission(p) if p.request.id == request.id));
        if exists.is_some() {
            return false;
        }
        self.push(BlockKind::Permission(PermissionBlock { request, state: PermissionState::Pending }));
        true
    }

    /// Marks a permission request answered; `false` when it is not in this transcript.
    pub(crate) fn resolve(&mut self, id: &RequestId) -> bool {
        let found = self.rfind(|kind| matches!(kind, BlockKind::Permission(p) if &p.request.id == id));
        match found {
            Some(index) => {
                self.edit(index, |kind| {
                    if let BlockKind::Permission(p) = kind {
                        p.state = PermissionState::Resolved;
                    }
                });
                true
            }
            None => false,
        }
    }

    /// Closes everything still streaming (a turn ended).
    fn close_streaming(&mut self) {
        let from = self.turn_start();
        for index in from..self.blocks.len() {
            let streaming = matches!(
                &self.blocks[index].kind,
                BlockKind::Assistant(text) | BlockKind::Thinking(text) if text.streaming
            );
            if streaming {
                self.edit(index, |kind| {
                    if let BlockKind::Assistant(text) | BlockKind::Thinking(text) = kind {
                        text.streaming = false;
                    }
                });
            }
        }
    }

    /// Splices `other`'s blocks in front of this transcript's, giving them fresh ids.
    fn prepend(&mut self, other: Transcript) -> Splice {
        let count = other.blocks.len();
        let mut front: Vec<Arc<Block>> = Vec::with_capacity(count + self.blocks.len());
        for block in other.blocks {
            let id = self.next_id;
            self.next_id += 1;
            front.push(Arc::new(Block { id, kind: block.kind.clone() }));
        }
        front.append(&mut self.blocks);
        self.blocks = front;
        self.orphans = other.orphans;
        if other.oldest_compacted.is_some() {
            self.oldest_compacted = other.oldest_compacted;
        }
        Splice { old: 0..0, count }
    }
}
