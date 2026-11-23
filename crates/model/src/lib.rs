//! Plain data types shared by every layer: sessions, issues, runnables, graphs, runs, events.
//!
//! Nothing here does IO or holds behaviour beyond small pure helpers. The store persists these
//! types, the engine produces them, the hub hands them to screens. A screen never defines its own
//! copy of a run or a session.

pub mod chat;
pub mod event;
pub mod graph;
pub mod ids;
pub mod inbox;
pub mod issue;
pub mod review;
pub mod run;
pub mod runnable;
pub mod spend;
pub mod workspace;

pub use chat::*;
pub use event::*;
pub use graph::*;
pub use ids::*;
pub use inbox::*;
pub use issue::*;
pub use review::*;
pub use run::*;
pub use runnable::*;
pub use spend::*;
pub use workspace::*;

/// Unix epoch milliseconds. Every timestamp in the model uses this.
pub type Millis = i64;
