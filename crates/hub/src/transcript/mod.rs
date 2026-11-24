//! A chat transcript as a flat, append-mostly list of blocks a view renders with GPUI's
//! `list()`. Blocks are built from stored messages (history) and from the live
//! [`AgentEvent`](crate::api::AgentEvent) stream; every change is reported as a [`Splice`] so the
//! view re-measures only what changed.

mod history;
mod live;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use crate::api::{
    Attachment, Millis, ModelRef, PermissionRequest, RequestId, SessionId, TodoItem, ToolClass, ToolDisplay,
};

pub(crate) use history::tool_call_ids;
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
    /// When the call started: the live event's arrival, or its stored message's time in
    /// history. `None` when nothing tells.
    pub started: Option<Millis>,
    /// When its result arrived; `None` while it runs or when nothing tells.
    pub finished: Option<Millis>,
}

impl ToolBlock {
    /// How long the call took, once both ends are known.
    pub fn duration_ms(&self) -> Option<Millis> {
        Some((self.finished? - self.started?).max(0))
    }
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
    /// The depth limit of the spawning agent's policy, when known: "depth 1 of 2".
    pub max_depth: Option<u32>,
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

/// Rows changed since the last report: edited ones below `base` (the length when the first
/// change came), appended ones from `base` on.
#[derive(Debug)]
struct Touched {
    base: usize,
    edited: BTreeSet<usize>,
}

/// The blocks plus what is needed to keep building them.
#[derive(Debug, Default)]
pub struct Transcript {
    blocks: Vec<Arc<Block>>,
    next_id: BlockId,
    /// What changed since the last `take_splices`.
    touched: Option<Touched>,
    /// Tool results whose call is not loaded (yet): id → (output, is_error, stored at).
    orphans: HashMap<String, (String, bool, Option<Millis>)>,
    /// The spawn policy's depth limit, given to every sub-agent row.
    max_depth: Option<u32>,
    /// The highest stored message seq applied.
    newest_seq: Option<u64>,
    /// Compacted messages seen since the last divider.
    compacted_run: u32,
    /// Whether the oldest loaded message was compacted.
    oldest_compacted: Option<bool>,
    /// What finished calls showed live, kept by the engine: history draws these instead of
    /// a display derived from the input. By tool-use id.
    kept_displays: HashMap<String, ToolDisplay>,
    /// Calls answered as interrupted (by a stop, a crash or a restart), by tool-use id.
    interrupted: HashSet<String>,
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

    /// Whether the call `id` never finished: it was answered as interrupted, by a stop or
    /// after a restart. Its row keeps the look of its kind; the result is not its output.
    pub fn is_interrupted(&self, id: &str) -> bool {
        self.interrupted.contains(id)
    }

    /// The highest stored message seq in the transcript.
    pub fn newest_seq(&self) -> Option<u64> {
        self.newest_seq
    }

    /// Records that block `index` (or the end, for a push) is about to change.
    fn touch(&mut self, index: usize) {
        let len = self.blocks.len();
        let touched = self.touched.get_or_insert_with(|| Touched { base: len, edited: BTreeSet::new() });
        if index < touched.base {
            touched.edited.insert(index);
        }
    }

    fn push(&mut self, kind: BlockKind) {
        self.touch(self.blocks.len());
        let id = self.next_id;
        self.next_id += 1;
        self.blocks.push(Arc::new(Block { id, kind }));
    }

    /// Changes a block; an edit that leaves it as it was is not reported.
    fn edit(&mut self, index: usize, f: impl FnOnce(&mut BlockKind)) {
        let Some(block) = self.blocks.get(index) else { return };
        let mut kind = block.kind.clone();
        f(&mut kind);
        if kind != block.kind {
            self.touch(index);
            Arc::make_mut(&mut self.blocks[index]).kind = kind;
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

    /// What changed since the last call, in order: one splice per run of adjacent edited rows,
    /// then one for the rows appended at the end. Rows in between are left alone, so a list
    /// keeps its scroll anchor unless that very row changed.
    pub(crate) fn take_splices(&mut self) -> Vec<Splice> {
        let Some(Touched { base, edited }) = self.touched.take() else { return Vec::new() };
        let mut splices: Vec<Splice> = Vec::new();
        for index in edited {
            match splices.last_mut() {
                Some(last) if last.old.end == index => {
                    last.old.end += 1;
                    last.count += 1;
                }
                _ => splices.push(Splice { old: index..index + 1, count: 1 }),
            }
        }
        if self.blocks.len() > base {
            splices.push(Splice { old: base..base, count: self.blocks.len() - base });
        }
        splices
    }

    /// Sets the spawn policy's depth limit on every sub-agent row, now and later.
    pub(crate) fn set_max_depth(&mut self, max_depth: Option<u32>) {
        self.max_depth = max_depth;
        for index in 0..self.blocks.len() {
            self.edit(index, |kind| {
                if let BlockKind::Subagent(sub) = kind {
                    sub.max_depth = max_depth;
                }
            });
        }
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
        for block in &mut self.blocks {
            if let BlockKind::Subagent(sub) = &block.kind
                && sub.max_depth != self.max_depth
            {
                let max_depth = self.max_depth;
                if let BlockKind::Subagent(sub) = &mut Arc::make_mut(block).kind {
                    sub.max_depth = max_depth;
                }
            }
        }
        if other.oldest_compacted.is_some() {
            self.oldest_compacted = other.oldest_compacted;
        }
        Splice { old: 0..0, count }
    }
}
