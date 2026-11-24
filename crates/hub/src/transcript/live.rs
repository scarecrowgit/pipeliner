//! Blocks from the live agent stream.

use crate::api::{AgentEvent, ContentBlock, Millis, Role, StopReason, StoredMessage, ToolDisplay};

use super::history::{ToolRole, tool_role};
use super::{
    BlockKind, CompactionBlock, NoticeBlock, NoticeKind, SubagentBlock, SubagentState, TextBlock, ToolBlock, ToolState,
    Transcript,
};

/// What an agent event means beyond the blocks it changed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiveEffect {
    TurnStarted,
    TurnFinished(StopReason),
    Failed(String),
}

/// The text a finished call's display carries as its result.
fn display_text(display: &ToolDisplay) -> &str {
    match display {
        ToolDisplay::Text { body, .. } => body,
        ToolDisplay::Command { output, .. } => output,
        ToolDisplay::Line { result, .. } => result,
        ToolDisplay::Diff(_) => "",
    }
}

impl Transcript {
    /// Applies one live event that arrived at `now`. Events about the session rather than the
    /// conversation (usage, files, skills) change no block.
    pub(crate) fn apply(&mut self, event: &AgentEvent, now: Millis) -> Option<LiveEffect> {
        match event {
            AgentEvent::TurnStarted => return Some(LiveEffect::TurnStarted),
            AgentEvent::TextDelta { text } => self.stream(text, false),
            AgentEvent::ThinkingDelta { text } => self.stream(text, true),
            AgentEvent::MessageCommitted { message } => self.commit(message),
            AgentEvent::ToolCallStarted { id, name } => {
                if tool_role(name) == ToolRole::Plain && self.tool_index(id).is_none() {
                    self.push(BlockKind::Tool(ToolBlock {
                        id: id.clone(),
                        name: name.clone(),
                        class: None,
                        display: None,
                        state: ToolState::Started,
                        started: Some(now),
                        finished: None,
                    }));
                }
            }
            AgentEvent::ToolCallReady { id, name, class, display } => {
                if tool_role(name) != ToolRole::Plain {
                    return None;
                }
                match self.tool_index(id) {
                    Some(index) => self.edit(index, |kind| {
                        if let BlockKind::Tool(tool) = kind {
                            tool.class = Some(*class);
                            tool.display = Some(display.clone());
                            tool.started.get_or_insert(now);
                            if tool.state == ToolState::Started {
                                tool.state = ToolState::Ready;
                            }
                        }
                    }),
                    None => self.push(BlockKind::Tool(ToolBlock {
                        id: id.clone(),
                        name: name.clone(),
                        class: Some(*class),
                        display: Some(display.clone()),
                        state: ToolState::Ready,
                        started: Some(now),
                        finished: None,
                    })),
                }
            }
            AgentEvent::ToolFinished { id, display, is_error } => {
                self.note_interrupted(id, *is_error, display_text(display));
                if let Some(index) = self.tool_index(id) {
                    self.edit(index, |kind| {
                        if let BlockKind::Tool(tool) = kind {
                            tool.display = Some(display.clone());
                            tool.state = ToolState::Finished { is_error: *is_error };
                            tool.finished.get_or_insert(now);
                        }
                    });
                }
            }
            AgentEvent::TodoUpdated { items } => {
                let start = self.turn_start();
                match self.rfind(|kind| matches!(kind, BlockKind::Todos(_))).filter(|&i| i >= start) {
                    Some(index) => self.edit(index, |kind| *kind = BlockKind::Todos(items.clone())),
                    None => self.push(BlockKind::Todos(items.clone())),
                }
            }
            AgentEvent::SubagentStarted { session, name, task, depth } => {
                let known = self.rfind(|kind| matches!(kind, BlockKind::Subagent(s) if s.session.as_ref() == Some(session)));
                if known.is_none() {
                    self.push(BlockKind::Subagent(SubagentBlock {
                        session: Some(session.clone()),
                        call: None,
                        name: name.clone(),
                        task: task.clone(),
                        depth: *depth,
                        state: SubagentState::Running,
                        max_depth: self.max_depth,
                    }));
                }
            }
            AgentEvent::SubagentFinished { session, summary, ok, tool_calls, secs, cost_usd } => {
                let found = self.rfind(|kind| matches!(kind, BlockKind::Subagent(s) if s.session.as_ref() == Some(session)));
                if let Some(index) = found {
                    self.edit(index, |kind| {
                        if let BlockKind::Subagent(sub) = kind {
                            sub.state = SubagentState::Finished {
                                ok: *ok,
                                summary: summary.clone(),
                                tool_calls: *tool_calls,
                                secs: *secs,
                                cost_usd: *cost_usd,
                            };
                        }
                    });
                }
            }
            AgentEvent::CompactionStarted => {
                self.push(BlockKind::Compaction(CompactionBlock { summarised: 0, running: true }));
            }
            AgentEvent::Compacted { summarised, .. } => {
                let done = CompactionBlock { summarised: *summarised, running: false };
                match self.rfind(|kind| matches!(kind, BlockKind::Compaction(c) if c.running)) {
                    Some(index) => self.edit(index, |kind| *kind = BlockKind::Compaction(done)),
                    None => self.push(BlockKind::Compaction(done)),
                }
            }
            AgentEvent::Retrying { attempt, reason } => self.notice(NoticeKind::Retry, format!("Retrying (attempt {attempt}): {reason}")),
            AgentEvent::PrDetected { url } => self.notice(NoticeKind::PullRequest, url.clone()),
            AgentEvent::Notice { text } => self.notice(NoticeKind::Info, text.clone()),
            AgentEvent::TurnFinished { stop } => {
                self.close_streaming();
                if *stop == StopReason::Cancelled {
                    self.notice(NoticeKind::Interrupted, "Interrupted".to_string());
                }
                return Some(LiveEffect::TurnFinished(*stop));
            }
            AgentEvent::Failed { error } => {
                self.close_streaming();
                self.push(BlockKind::Failure(error.clone()));
                return Some(LiveEffect::Failed(error.clone()));
            }
            AgentEvent::Usage { .. }
            | AgentEvent::FilesChanged { .. }
            | AgentEvent::SkillLoaded { .. } => {}
        }
        None
    }

    fn notice(&mut self, kind: NoticeKind, text: String) {
        self.push(BlockKind::Notice(NoticeBlock { kind, text }));
    }

    /// Appends a delta to the streaming block of its kind at the end, or starts one.
    fn stream(&mut self, delta: &str, thinking: bool) {
        let last = self.blocks.len().checked_sub(1);
        let open = last.filter(|&i| match &self.blocks[i].kind {
            BlockKind::Assistant(text) => !thinking && text.streaming,
            BlockKind::Thinking(text) => thinking && text.streaming,
            _ => false,
        });
        match open {
            Some(index) => self.edit(index, |kind| {
                if let BlockKind::Assistant(text) | BlockKind::Thinking(text) = kind {
                    text.text.push_str(delta);
                }
            }),
            None => {
                let block = TextBlock { seq: None, at: None, model: None, text: delta.to_string(), streaming: true };
                self.push(if thinking { BlockKind::Thinking(block) } else { BlockKind::Assistant(block) });
            }
        }
    }

    /// A message stored mid-turn: closes the blocks streamed for it, adds what did not stream.
    fn commit(&mut self, stored: &StoredMessage) {
        if self.newest_seq.is_some_and(|seq| stored.seq <= seq) {
            return;
        }
        if stored.message.role == Role::User {
            self.push_stored(stored);
            return;
        }
        self.newest_seq = Some(stored.seq);
        let start = self.turn_start();
        let mut streamed_text = false;
        for index in start..self.blocks.len() {
            let streaming = matches!(
                &self.blocks[index].kind,
                BlockKind::Assistant(text) | BlockKind::Thinking(text) if text.streaming
            );
            if !streaming {
                continue;
            }
            streamed_text |= matches!(&self.blocks[index].kind, BlockKind::Assistant(_));
            self.edit(index, |kind| {
                if let BlockKind::Assistant(text) | BlockKind::Thinking(text) = kind {
                    text.streaming = false;
                    text.seq = Some(stored.seq);
                    text.at = Some(stored.at);
                    text.model = stored.model.clone();
                }
            });
        }
        let text = stored.message.plain_text();
        if !streamed_text && !text.is_empty() {
            self.push(BlockKind::Assistant(TextBlock {
                seq: Some(stored.seq),
                at: Some(stored.at),
                model: stored.model.clone(),
                text,
                streaming: false,
            }));
        }
        for block in &stored.message.content {
            if let ContentBlock::ToolUse { id, name, .. } = block
                && tool_role(name) == ToolRole::Plain
            {
                self.tool_use(id, name, block, Some(stored.at));
            }
        }
    }
}
