//! Pipeline graphs: validation, diff, the executor, node runners and reviews.
//!
//! | Module | What it gives |
//! | --- | --- |
//! | [`api`] | The contract: [`Job`], [`JobResult`], [`NodeRunner`], [`Item`] |
//! | [`exec`] | The executor: the state machine of one run and the threads that drive it |
//! | [`fake`] | A scripted [`NodeRunner`] for tests of this crate and of the engine |
//! | [`expr`] | Router conditions: parsing and evaluation over an [`Item`] |
//! | [`validate`] | The editor's Problems list for a graph |
//! | [`diff`] | "Changes since v14": what differs between two graphs |
//! | [`legacy`] | Conversion of the old app's linear pipelines into graphs |

pub mod api;
pub mod diff;
pub mod exec;
pub mod expr;
pub mod fake;
pub mod legacy;
pub mod validate;

pub use api::{
    InstanceKey, Item, Job, JobNote, JobResult, JobSink, NodeOutput, NodeRunner, Retry, SpawnedState, Upstream,
};
