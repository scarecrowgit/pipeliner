//! Issue tracker rules: statuses, start triggers, mention wake-ups, board view rules and
//! autopilot schedules.
//!
//! Everything here is pure: no IO, no threads, no clock. Times come in as arguments. Storage,
//! starting agents and the screens call these functions and act on what they return.

pub mod agents;
pub mod autopilot;
pub mod board;
pub mod card;
pub mod keys;
pub mod mentions;
pub mod status;
pub mod triggers;

#[cfg(test)]
mod testing;

pub use board::{Board, Chip, Column, ListColumn, RunLookup, RunsWith, SortDir};
pub use card::CardState;
pub use mentions::{Completion, CompletionItem, Mention};
pub use status::{Effect, RunResult};
pub use agents::{run_agents, wake_message};
pub use triggers::{Happening, WakeIntent, WakeMode, WakeRefusal, Wakes};
