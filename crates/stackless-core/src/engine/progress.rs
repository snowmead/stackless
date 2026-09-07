//! Step progress events during `up` — substrate-agnostic telemetry for
//! agents and human operators.

use super::plan::StepKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepProgressEvent {
    Started,
    Skipped,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepProgress {
    pub event: StepProgressEvent,
    pub instance: String,
    pub step_id: String,
    pub step_kind: StepKind,
    pub node: String,
    /// 1-based index within the plan.
    pub index: usize,
    pub total: usize,
    /// Set on [`StepProgressEvent::Failed`] when a stable code is known.
    pub code: Option<String>,
    /// Wall-clock time of this event (Unix epoch milliseconds).
    pub at_epoch_ms: i64,
    /// Elapsed time since `step_started`, set on completed/failed/skipped.
    pub duration_ms: Option<u64>,
}

pub trait ProgressSink {
    fn on_step(&mut self, progress: StepProgress);
    /// Called after admission, before any provisioning side effect.
    fn on_admitted(&mut self, _record: &crate::state::InstanceRecord) {}
    /// Cancellation is honored between steps, after resource handles are durable.
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct NullProgress;

impl ProgressSink for NullProgress {
    fn on_step(&mut self, _progress: StepProgress) {}
}

pub fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
