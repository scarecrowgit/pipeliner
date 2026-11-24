//! Toast-able messages the hub emits: failed commands and the engine's own notices.

use gpui::SharedString;

use crate::api::NoticeLevel;

/// Something to show as a toast. The [`Hub`](crate::Hub) emits it; the shell subscribes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubNotice {
    pub level: NoticeLevel,
    /// One sentence: "Could not pin the chat".
    pub text: SharedString,
    /// The cause, for a second line or a details popover.
    pub detail: Option<SharedString>,
}

impl HubNotice {
    /// A notice with no detail line.
    pub fn new(level: NoticeLevel, text: impl Into<SharedString>) -> Self {
        Self { level, text: text.into(), detail: None }
    }

    /// A command failed: "Could not {action}", with the error chain as the detail.
    pub fn failed(action: &str, error: &anyhow::Error) -> Self {
        Self {
            level: NoticeLevel::Error,
            text: format!("Could not {action}").into(),
            detail: Some(format!("{error:#}").into()),
        }
    }
}
