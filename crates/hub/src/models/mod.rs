//! The model entities views observe, one per area of the app.

mod catalog;
mod chat_index;
mod inbox;
mod issues;
mod ops;
mod permissions;
mod runs;
mod search;
mod session;

pub use catalog::CatalogModel;
pub use chat_index::ChatIndex;
pub use inbox::{InboxModel, ReviewModel};
pub use issues::IssuesModel;
pub use ops::{OpsModel, TOP_SPENDERS};
pub use permissions::PermissionsModel;
pub use runs::{LOG_CAPACITY, LOG_TAIL, RunLogEvent, RunModel, RunsModel};
pub use search::{DEBOUNCE, SearchModel};
pub use session::{ContextUsage, PAGE, SessionEvent, SessionModel};

pub(crate) use chat_index::IndexInput;
pub(crate) use inbox::InboxInput;
pub(crate) use issues::IssueInput;
pub(crate) use permissions::PermissionInput;
pub(crate) use runs::RunInput;
pub(crate) use session::SessionInput;
