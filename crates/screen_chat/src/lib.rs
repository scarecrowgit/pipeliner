//! The chat screen: a session's transcript with its composer and context panel, the new-chat
//! page, and the list of every chat.
//!
//! The crate gives the app three views, one per route, and nothing else; the shell mounts
//! them and draws their header.
//!
//! | Route | View | Module |
//! | --- | --- | --- |
//! | `Route::Chat { session }` | [`SessionView`] | [`session`], with [`transcript`] inside |
//! | `Route::NewChat` | [`NewChatView`] | [`new_chat`] |
//! | `Route::Chats` | [`ChatsView`] | [`new_chat`] |
//!
//! [`workdir`] holds the folder chip, its popover and the switch dialog, which both the
//! session view and the new-chat page use.
//!
//! Everything shown comes from `hub` models; everything done goes through `hub::commands`.

pub mod new_chat;
pub mod session;
pub mod transcript;
pub mod workdir;

pub use new_chat::{ChatsView, NewChatView};
pub use session::SessionView;
pub use transcript::TranscriptView;
pub use workdir::WorkdirChip;
