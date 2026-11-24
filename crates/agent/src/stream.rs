//! One model response as it streams in: the content blocks of the assistant message and the
//! tool calls to run.

use llm::StreamEvent;
use model::{ContentBlock, StopReason, Usage};
use serde_json::Value;

/// A tool call the model made.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Call {
    /// Unique within its message, also when the provider sent no id or the same id twice.
    pub id: String,
    pub name: String,
    pub input: Value,
    /// Why the model's JSON could not be read; such a call is answered, not run.
    pub error: Option<String>,
}

/// A finished (or cut off) model response.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reply {
    /// Text, thinking and complete tool calls, in the order they arrived.
    pub content: Vec<ContentBlock>,
    pub calls: Vec<Call>,
    pub usage: Option<Usage>,
    pub stop: Option<StopReason>,
    /// What the call cost; filled in once the usage is accounted.
    pub cost_usd: f64,
}

impl Reply {
    /// The text blocks joined.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            if let ContentBlock::Text { text } = block {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        out
    }

    /// Whether there is anything to store: text or a tool call (thinking alone is not).
    pub fn has_content(&self) -> bool {
        self.content.iter().any(|b| matches!(b, ContentBlock::Text { .. } | ContentBlock::ToolUse { .. }))
    }
}

/// What a stream event means for whoever watches the run.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Live {
    Text(String),
    Thinking(String),
    ToolStarted { id: String, name: String },
    /// The call's input is complete.
    ToolReady(Call),
}

/// Builds a [`Reply`] from stream events.
#[derive(Default)]
pub(crate) struct Collector {
    content: Vec<ContentBlock>,
    calls: Vec<Call>,
    usage: Option<Usage>,
    stop: Option<StopReason>,
    /// Calls that started: the provider's id, ours, and whether the input has arrived.
    started: Vec<(String, String, bool)>,
    /// Ids of tool calls already in the conversation; a new call never gets one of them.
    in_use: Vec<String>,
}

impl Collector {
    /// A collector whose calls get ids different from `in_use`, the ids of the calls the
    /// conversation already holds. Providers that send no ids, or count from zero in every
    /// response, would otherwise repeat them, and some providers reject a repeated id.
    pub fn avoiding(in_use: Vec<String>) -> Self {
        Self { in_use, ..Self::default() }
    }

    pub fn stop(&self) -> Option<StopReason> {
        self.stop
    }

    /// Takes one event in; returns what to show for it.
    pub fn on_event(&mut self, event: StreamEvent) -> Vec<Live> {
        match event {
            StreamEvent::TextDelta(delta) => {
                match self.content.last_mut() {
                    Some(ContentBlock::Text { text }) => text.push_str(&delta),
                    _ => self.content.push(ContentBlock::Text { text: delta.clone() }),
                }
                vec![Live::Text(delta)]
            }
            StreamEvent::ThinkingDelta(delta) => {
                match self.content.last_mut() {
                    Some(ContentBlock::Thinking { text, signature: None }) => text.push_str(&delta),
                    _ => self.content.push(ContentBlock::Thinking { text: delta.clone(), signature: None }),
                }
                vec![Live::Thinking(delta)]
            }
            StreamEvent::ThinkingSignature(value) => {
                let unsigned = self
                    .content
                    .iter_mut()
                    .rev()
                    .find(|b| matches!(b, ContentBlock::Thinking { signature: None, .. }));
                if let Some(ContentBlock::Thinking { signature, .. }) = unsigned {
                    *signature = Some(value);
                }
                Vec::new()
            }
            StreamEvent::ToolUseStart { id, name } => {
                let ours = self.start(&id);
                vec![Live::ToolStarted { id: ours, name }]
            }
            StreamEvent::ToolUseInputDelta { .. } => Vec::new(),
            StreamEvent::ToolUse { id, name, input, error } => {
                let mut live = Vec::new();
                let waiting = self.started.iter_mut().find(|(theirs, _, done)| *theirs == id && !done);
                let ours = match waiting {
                    Some((_, ours, done)) => {
                        *done = true;
                        ours.clone()
                    }
                    None => {
                        // A client that reports the whole call at once.
                        let ours = self.start(&id);
                        if let Some(entry) = self.started.last_mut() {
                            entry.2 = true;
                        }
                        live.push(Live::ToolStarted { id: ours.clone(), name: name.clone() });
                        ours
                    }
                };
                let input = if input.is_object() { input } else { Value::Object(Default::default()) };
                self.content.push(ContentBlock::ToolUse { id: ours.clone(), name: name.clone(), input: input.clone() });
                let call = Call { id: ours, name, input, error };
                self.calls.push(call.clone());
                live.push(Live::ToolReady(call));
                live
            }
            StreamEvent::Usage(usage) => {
                self.usage = Some(usage);
                Vec::new()
            }
            StreamEvent::Stop(stop) => {
                self.stop = Some(stop);
                Vec::new()
            }
        }
    }

    /// Registers a started call and returns the id it goes by here.
    fn start(&mut self, theirs: &str) -> String {
        let taken =
            |id: &str| self.started.iter().any(|(_, ours, _)| ours == id) || self.in_use.iter().any(|used| used == id);
        let ours = if theirs.is_empty() || taken(theirs) {
            let mut n = self.started.len() + 1;
            while taken(&format!("call_{n}")) {
                n += 1;
            }
            format!("call_{n}")
        } else {
            theirs.to_string()
        };
        self.started.push((theirs.to_string(), ours.clone(), false));
        ours
    }

    /// The response so far. Blank text is dropped; calls whose input never completed are not
    /// part of it.
    pub fn finish(self) -> Reply {
        let content = self
            .content
            .into_iter()
            .filter(|b| !matches!(b, ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } if text.trim().is_empty()))
            .collect();
        Reply { content, calls: self.calls, usage: self.usage, stop: self.stop, cost_usd: 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn feed(events: Vec<StreamEvent>) -> (Reply, Vec<Live>) {
        let mut collector = Collector::default();
        let mut live = Vec::new();
        for event in events {
            live.extend(collector.on_event(event));
        }
        (collector.finish(), live)
    }

    #[test]
    fn blocks_keep_their_order_and_signatures() {
        let (reply, live) = feed(vec![
            StreamEvent::ThinkingDelta("Hm, ".into()),
            StreamEvent::ThinkingDelta("read it.".into()),
            StreamEvent::ThinkingSignature("sig".into()),
            StreamEvent::TextDelta("Reading ".into()),
            StreamEvent::TextDelta("it.".into()),
            StreamEvent::ToolUseStart { id: "t1".into(), name: "read_file".into() },
            StreamEvent::ToolUseInputDelta { id: "t1".into(), json: "{}".into() },
            StreamEvent::ToolUse { id: "t1".into(), name: "read_file".into(), input: json!({"path": "a"}), error: None },
            StreamEvent::TextDelta(" ".into()),
            StreamEvent::Usage(Usage { input: 5, output: 2, ..Default::default() }),
            StreamEvent::Stop(StopReason::ToolUse),
        ]);
        assert_eq!(
            reply.content,
            vec![
                ContentBlock::Thinking { text: "Hm, read it.".into(), signature: Some("sig".into()) },
                ContentBlock::text("Reading it."),
                ContentBlock::ToolUse { id: "t1".into(), name: "read_file".into(), input: json!({"path": "a"}) },
            ]
        );
        assert_eq!(reply.calls.len(), 1);
        assert_eq!(reply.text(), "Reading it.");
        assert_eq!(reply.stop, Some(StopReason::ToolUse));
        assert_eq!(reply.usage.map(|u| u.input), Some(5));
        assert_eq!(live.len(), 7);
        assert!(matches!(&live[4], Live::ToolStarted { id, .. } if id == "t1"));
        assert!(matches!(&live[5], Live::ToolReady(call) if call.id == "t1"));
    }

    #[test]
    fn missing_and_repeated_ids_are_made_unique() {
        let tool = |id: &str| StreamEvent::ToolUse { id: id.into(), name: "x".into(), input: json!({}), error: None };
        let (reply, _) = feed(vec![
            StreamEvent::ToolUseStart { id: "".into(), name: "x".into() },
            tool(""),
            StreamEvent::ToolUseStart { id: "a".into(), name: "x".into() },
            StreamEvent::ToolUseStart { id: "a".into(), name: "x".into() },
            tool("a"),
            tool("a"),
            tool("late"),
        ]);
        let ids: Vec<&str> = reply.calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["call_1", "a", "call_3", "late"]);
    }

    #[test]
    fn ids_already_in_the_conversation_are_not_given_again() {
        let mut collector = Collector::avoiding(vec!["call_0".into(), "call_1".into()]);
        for id in ["call_0", "", "fresh"] {
            collector.on_event(StreamEvent::ToolUseStart { id: id.into(), name: "x".into() });
            collector.on_event(StreamEvent::ToolUse { id: id.into(), name: "x".into(), input: json!({}), error: None });
        }
        let ids: Vec<String> = collector.finish().calls.into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["call_2", "call_3", "fresh"]);
    }

    #[test]
    fn a_call_cut_off_before_its_input_is_not_in_the_reply() {
        let (reply, live) = feed(vec![
            StreamEvent::TextDelta("Let me".into()),
            StreamEvent::ToolUseStart { id: "t1".into(), name: "bash".into() },
            StreamEvent::ToolUseInputDelta { id: "t1".into(), json: "{\"comm".into() },
        ]);
        assert_eq!(reply.content, vec![ContentBlock::text("Let me")]);
        assert!(reply.calls.is_empty());
        assert!(reply.has_content());
        assert_eq!(live.len(), 2);
        assert!(!feed(vec![StreamEvent::ThinkingDelta("only thoughts".into())]).0.has_content());
    }

    #[test]
    fn input_that_is_not_an_object_becomes_empty() {
        let (reply, _) = feed(vec![StreamEvent::ToolUse {
            id: "t1".into(),
            name: "x".into(),
            input: json!("oops"),
            error: Some("bad".into()),
        }]);
        assert_eq!(reply.calls[0].input, json!({}));
        assert_eq!(reply.calls[0].error.as_deref(), Some("bad"));
    }
}
