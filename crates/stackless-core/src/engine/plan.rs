//! Step planning: one validated definition + the derived graph → the
//! ordered steps every substrate executes (§3/§4 share the sequence:
//! provision integrations → prepare → start services → health gate).

use serde::{Deserialize, Serialize};

use crate::def::{DefError, DependencyGraph, Node, StackDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Provision a hosted third-party integration (Clerk in v0).
    ProvisionIntegration,
    /// Materialize a service's source into instance-owned space.
    Materialize,
    /// The once-after-materialization hook.
    Setup,
    /// The every-up hook, after dependencies are ready.
    Prepare,
    /// Start (or deploy) the service.
    Start,
    /// Run a finite job and persist its exit status.
    RunJob,
    /// Gate on the service's health contract through its public origin.
    HealthGate,
}

impl StepKind {
    fn id_prefix(self) -> &'static str {
        match self {
            Self::ProvisionIntegration => "integration",
            Self::Materialize => "materialize",
            Self::Setup => "setup",
            Self::Prepare => "prepare",
            Self::Start => "start",
            Self::HealthGate => "health",
            Self::RunJob => "job",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Stable id, the journal's primary key: `"{kind}:{node}"`.
    pub id: String,
    pub kind: StepKind,
    /// The service name the step belongs to.
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

impl StackDef {
    /// Expand the topological order into lifecycle steps.
    pub fn plan(&self) -> Result<Vec<Step>, DefError> {
        let graph = DependencyGraph::derive(self)?;
        let mut steps = Vec::new();
        for node in graph.startup_order() {
            match node {
                Node::Integration(name) => {
                    steps.push(Step::new(StepKind::ProvisionIntegration, name));
                }
                Node::Service(name) => {
                    let Some(service) = self.services.get(name) else {
                        continue;
                    };
                    steps.push(Step::new(StepKind::Materialize, name));
                    if service.setup.is_some() {
                        steps.push(Step::new(StepKind::Setup, name));
                    }
                    if service.prepare.is_some() {
                        steps.push(Step::new(StepKind::Prepare, name));
                    }
                    if service.kind == crate::def::WorkloadKind::Job {
                        steps.push(Step::new(StepKind::RunJob, name));
                    } else {
                        steps.push(Step::new(StepKind::Start, name));
                        steps.push(Step::new(StepKind::HealthGate, name));
                    }
                }
            }
        }
        Ok(steps)
    }
}

/// Dependencies are on output production or readiness, never on list position.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionPlan {
    pub steps: Vec<Step>,
    pub dependencies: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

impl ExecutionPlan {
    pub fn ancestors(&self, id: &str) -> std::collections::BTreeSet<String> {
        let mut found = std::collections::BTreeSet::new();
        let mut pending = vec![id.to_owned()];
        while let Some(id) = pending.pop() {
            if let Some(dependencies) = self.dependencies.get(&id) {
                for dependency in dependencies {
                    if found.insert(dependency.clone()) {
                        pending.push(dependency.clone());
                    }
                }
            }
        }
        found
    }
}

impl StackDef {
    pub fn execution_plan(
        &self,
        provider: &str,
        early_origins: bool,
    ) -> Result<ExecutionPlan, DefError> {
        self.execution_plan_with_placement(provider, |_| early_origins)
    }

    /// URL availability is a property of the target's hosting provider.
    pub fn execution_plan_with_placement(
        &self,
        provider: &str,
        early_origins: impl Fn(&str) -> bool,
    ) -> Result<ExecutionPlan, DefError> {
        let early = |target: &str| {
            if self.services.get(target).is_some_and(|workload| {
                workload
                    .health
                    .as_ref()
                    .is_some_and(|health| health.is_tcp())
            }) {
                return false;
            }
            let provider = self
                .services
                .get(target)
                .and_then(|workload| workload.on.as_deref())
                .unwrap_or(provider);
            early_origins(provider)
        };
        use crate::def::{DependencyCondition, WorkloadKind, interp::Reference};
        use std::collections::{BTreeMap, BTreeSet};
        let steps = self.plan()?;
        let mut dependencies: BTreeMap<String, BTreeSet<String>> = steps
            .iter()
            .map(|step| (step.id.clone(), BTreeSet::new()))
            .collect();
        for (name, service) in &self.services {
            let chain: Vec<_> = steps
                .iter()
                .filter(|step| step.node == *name && step.kind != StepKind::ProvisionIntegration)
                .collect();
            for pair in chain.windows(2) {
                dependencies
                    .entry(pair[1].id.clone())
                    .or_default()
                    .insert(pair[0].id.clone());
            }
            let consumer = chain
                .iter()
                .find(|step| step.kind != StepKind::Materialize)
                .map(|step| step.id.clone());
            let Some(consumer) = consumer else {
                continue;
            };
            let inputs = dependencies.entry(consumer).or_default();
            for (target, condition) in &service.depends_on {
                let prefix = match condition {
                    DependencyCondition::Started => "start",
                    DependencyCondition::Ready => "health",
                    DependencyCondition::Completed => "job",
                };
                inputs.insert(format!("{prefix}:{target}"));
            }
            let env = service.effective_env(name, service.on.as_deref().unwrap_or(provider))?;
            for value in env.values() {
                for reference in
                    crate::def::interp::references(value, &format!("services.{name}.env"))?
                {
                    match reference {
                        Reference::IntegrationOutput { integration, .. } => {
                            inputs.insert(format!("integration:{integration}"));
                        }
                        Reference::ServiceOrigin(target) if !early(&target) => {
                            inputs.insert(format!("start:{target}"));
                        }
                        Reference::EndpointUrl(endpoint) => {
                            if let Some(endpoint) = self.endpoints.get(&endpoint)
                                && endpoint.url.is_none()
                                && !early(&endpoint.workload)
                            {
                                inputs.insert(format!("start:{}", endpoint.workload));
                            }
                        }
                        _ => {}
                    }
                }
            }
            if service.kind == WorkloadKind::Job && inputs.contains(&format!("job:{name}")) {
                return Err(DefError::WiringCycle {
                    nodes: name.clone(),
                });
            }
        }
        let mut remaining = dependencies.clone();
        let mut ordered = Vec::new();
        while !remaining.is_empty() {
            let next = remaining
                .iter()
                .find(|(_, deps)| deps.is_empty())
                .map(|(id, _)| id.clone())
                .ok_or_else(|| DefError::WiringCycle {
                    nodes: remaining.keys().cloned().collect::<Vec<_>>().join(", "),
                })?;
            remaining.remove(&next);
            for edges in remaining.values_mut() {
                edges.remove(&next);
            }
            if let Some(step) = steps.iter().find(|step| step.id == next) {
                ordered.push(step.clone());
            }
        }
        Ok(ExecutionPlan {
            steps: ordered,
            dependencies,
        })
    }
}

impl StepKind {
    /// Schedule starts before health waits so independent services can boot.
    pub fn priority(self) -> u8 {
        match self {
            Self::Materialize => 0,
            Self::ProvisionIntegration => 1,
            Self::Setup => 2,
            Self::Prepare => 3,
            Self::RunJob => 4,
            Self::Start => 5,
            Self::HealthGate => 6,
        }
    }
}
