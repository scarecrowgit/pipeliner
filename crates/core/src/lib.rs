//! Pipeliner core: domain model, persistent store and configuration.
//! No UI and no network code lives here.

pub mod config;
pub mod model;
pub mod paths;
pub mod skills;
pub mod store;

pub use config::Config;
pub use model::*;
pub use store::{Change, Store};
