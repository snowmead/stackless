//! The state store (ARCHITECTURE.md §2).

pub mod error;
mod instance;
mod journal;
mod lease;
mod lock;
mod store;

pub use error::StateError;
pub use instance::{InstanceRecord, InstanceStatus};
pub use journal::Checkpoint;
pub use lease::Lease;
pub use lock::LockClaim;
pub use store::{Store, state_dir};
