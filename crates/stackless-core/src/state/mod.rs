//! The state store (ARCHITECTURE.md §2).

pub mod error;
mod instance;
mod journal;
mod lease;
mod lock;
mod operation;
mod placement;
mod reaper;
mod resource;
mod revision;
mod row;
mod secrets;
mod store;
mod stripe;
mod value;

pub use error::StateError;
pub use instance::{InstanceRecord, InstanceStatus};
pub use journal::Checkpoint;
pub use lease::Lease;
pub use lock::LockClaim;
pub use operation::{Operation, OperationEvent, OperationStatus, new_operation_id};
pub use reaper::{ReapAttempt, ReapDecision, TOMBSTONE_GC_WINDOW};
pub use resource::{
    INSTANCE_RESOURCE_STEP, Ownership, ResourceIntent, ResourcePhase, ResourceRecord,
};
pub use revision::RevisionState;
pub use store::Store;
pub use stripe::StripeProjectRecord;

mod grants;
