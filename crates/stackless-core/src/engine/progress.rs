//! Step progress events during `up` — substrate-agnostic telemetry for
//! agents and human operators.

use super::plan::StepKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepProgressEvent {
    Started,
    Skipped,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
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
    pub code: Option<&'static str>,
}

pub trait ProgressSink {
    fn on_step(&mut self, progress: StepProgress);
}

#[derive(Debug, Default)]
pub struct NullProgress;

impl ProgressSink for NullProgress {
    fn on_step(&mut self, _progress: StepProgress) {}
}