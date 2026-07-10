//! The state store (ARCHITECTURE.md §2).

pub mod error;
mod instance;
mod journal;
mod lease;
mod lock;
mod reaper;
mod remote;
mod row;
mod store;
mod value;

pub use error::StateError;
pub use instance::{InstanceRecord, InstanceStatus};
pub use journal::Checkpoint;
pub use lease::Lease;
pub use lock::LockClaim;
pub use reaper::{ReapAttempt, ReapDecision, TOMBSTONE_GC_WINDOW};
pub use store::Store;
