//! Hosting placement is independent of the catalog provider for a resource.

use std::collections::BTreeMap;

use super::StackDef;
use crate::engine::{Step, StepKind};

/// Resolved hosting adapters. Catalog resource providers remain separate.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Placements {
    pub workloads: BTreeMap<String, String>,
    pub resources: BTreeMap<String, String>,
}

/// Stable resource lifetime key shared by planning and the placement journal.
pub fn placement_key(step: &str) -> Option<String> {
    let (kind, name) = step.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    match kind {
        "integration" => Some(format!("integration:{name}")),
        "materialize" | "setup" | "prepare" | "start" | "health" | "job" => {
            Some(format!("service:{name}"))
        }
        _ => None,
    }
}

impl StackDef {
    pub fn resolved_placements(&self, default_provider: &str) -> Placements {
        Placements {
            workloads: self
                .services
                .iter()
                .map(|(name, workload)| {
                    (
                        name.clone(),
                        workload.on.as_deref().unwrap_or(default_provider).into(),
                    )
                })
                .collect(),
            resources: self
                .integrations
                .iter()
                .map(|(name, resource)| {
                    (
                        name.clone(),
                        resource.on.as_deref().unwrap_or(default_provider).into(),
                    )
                })
                .collect(),
        }
    }

    pub fn placements(&self, default_provider: &str) -> BTreeMap<String, String> {
        self.services
            .iter()
            .map(|(name, workload)| {
                (
                    format!("service:{name}"),
                    workload
                        .on
                        .as_deref()
                        .unwrap_or(default_provider)
                        .to_owned(),
                )
            })
            .chain(self.integrations.iter().map(|(name, resource)| {
                (
                    format!("integration:{name}"),
                    resource
                        .on
                        .as_deref()
                        .unwrap_or(default_provider)
                        .to_owned(),
                )
            }))
            .collect()
    }

    pub fn step_provider<'a>(&'a self, default_provider: &'a str, step: &Step) -> &'a str {
        if step.kind == StepKind::ProvisionIntegration {
            self.integrations
                .get(&step.node)
                .and_then(|resource| resource.on.as_deref())
        } else {
            self.services
                .get(&step.node)
                .and_then(|workload| workload.on.as_deref())
        }
        .unwrap_or(default_provider)
    }
}
