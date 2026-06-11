//! Step planning: one validated definition + the derived graph → the
//! ordered steps every substrate executes (§3/§4 share the sequence:
//! provision datastores → prepare → start services → health gate).

use serde::Serialize;

use crate::def::{DefError, DependencyGraph, Node, StackDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Provision a datastore (container locally, managed service on a
    /// cloud substrate).
    ProvisionDatastore,
    /// Materialize a service's source into instance-owned space.
    Materialize,
    /// The once-after-materialization hook.
    Setup,
    /// The every-up hook, after dependencies are ready.
    Prepare,
    /// Start (or deploy) the service.
    Start,
    /// Gate on the service's health contract through its public origin.
    HealthGate,
}

impl StepKind {
    fn id_prefix(self) -> &'static str {
        match self {
            Self::ProvisionDatastore => "provision",
            Self::Materialize => "materialize",
            Self::Setup => "setup",
            Self::Prepare => "prepare",
            Self::Start => "start",
            Self::HealthGate => "health",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    /// Stable id, the journal's primary key: `"{kind}:{node}"`.
    pub id: String,
    pub kind: StepKind,
    /// The service or datastore name the step belongs to.
    pub node: String,
}

impl Step {
    fn new(kind: StepKind, node: &str) -> Self {
        Self {
            id: format!("{}:{node}", kind.id_prefix()),
            kind,
            node: node.to_owned(),
        }
    }
}

/// Expand the topological order into steps. Dependencies come first in
/// the order, so per-node expansion preserves the §4 sequencing
/// contract (a service's `prepare` runs after its datastores exist).
pub fn plan(def: &StackDef) -> Result<Vec<Step>, DefError> {
    let graph = DependencyGraph::derive(def)?;
    let mut steps = Vec::new();
    for node in graph.startup_order() {
        match node {
            Node::Datastore(name) => {
                steps.push(Step::new(StepKind::ProvisionDatastore, name));
            }
            Node::Service(name) => {
                let Some(service) = def.services.get(name) else {
                    continue;
                };
                steps.push(Step::new(StepKind::Materialize, name));
                if service.setup.is_some() {
                    steps.push(Step::new(StepKind::Setup, name));
                }
                if service.prepare.is_some() {
                    steps.push(Step::new(StepKind::Prepare, name));
                }
                steps.push(Step::new(StepKind::Start, name));
                steps.push(Step::new(StepKind::HealthGate, name));
            }
        }
    }
    Ok(steps)
}
