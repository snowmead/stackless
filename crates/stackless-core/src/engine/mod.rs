//! The lifecycle engine (ARCHITECTURE.md §2, §8).

pub mod error;
pub mod plan;
pub mod run;

pub use error::EngineError;
pub use plan::{Step, StepKind, plan};
pub use run::{DownOutcome, Engine, UpOutcome, UpRequest};
