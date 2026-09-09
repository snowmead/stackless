//! The lifecycle engine (ARCHITECTURE.md §2, §8).

pub mod error;
pub mod plan;
pub mod progress;
pub mod revision;
pub mod run;
mod teardown;

pub use error::EngineError;
pub use plan::{Step, StepKind};
pub use progress::{NullProgress, ProgressSink, StepProgress, StepProgressEvent, epoch_ms};
pub use run::{DownOutcome, Engine, StepTiming, UpAdmission, UpOutcome, UpRequest};
