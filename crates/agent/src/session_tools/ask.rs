//! `ask_user`: a question that stops the run until a person answers.
//!
//! The call is answered at once, so the conversation stays valid, and the run ends with
//! [`crate::StopCause::Waiting`]. The person's answer arrives as the next user message. No
//! thread waits in between, so a question survives the app closing.

use model::ToolSpec;
use serde_json::{Value, json};

use super::names::ASK_USER;
use super::{object, required_str};

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        name: ASK_USER.into(),
        description: "Ask the person one precise question when you cannot go on without their decision or \
                      information only they have. Your work stops until they answer; their answer arrives as \
                      the next message. Do not use it for things you can find out with your tools."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The question, with the options you see if there are any" }
            },
            "required": ["question"]
        }),
    }
}

/// The question in the input.
pub(crate) fn parse(input: &Value) -> Result<String, String> {
    let map = object(ASK_USER, input)?;
    required_str(ASK_USER, &map, "question").map(str::to_string)
}

/// What the model reads back.
pub(crate) const ASKED: &str = "The question was sent to the person. Stop here; their answer arrives as the next message.";
