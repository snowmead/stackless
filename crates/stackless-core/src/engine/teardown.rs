//! Teardown follows recorded resource generations. Step edges cover legacy checkpoints only.

use std::collections::{BTreeMap, BTreeSet};

use crate::state::{Checkpoint, INSTANCE_RESOURCE_STEP, ResourceRecord, StateError};
use crate::substrate::ACTION_RESOURCE_KIND;

use super::plan::ExecutionPlan;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Node {
    Resource(usize),
    Legacy(usize),
}

pub(super) struct Teardown {
    pub order: Vec<Node>,
    pub parents: BTreeMap<Node, BTreeSet<Node>>,
}

impl Teardown {
    pub fn build(
        resources: &[ResourceRecord],
        checkpoints: &[Checkpoint],
        plan: &ExecutionPlan,
    ) -> Result<Self, StateError> {
        let mut parents = BTreeMap::<Node, BTreeSet<Node>>::new();
        let mut by_step = BTreeMap::<&str, Vec<Node>>::new();
        let keys: BTreeMap<_, _> = resources
            .iter()
            .enumerate()
            .map(|(index, resource)| (resource.key.as_str(), Node::Resource(index)))
            .collect();
        for (index, resource) in resources.iter().enumerate() {
            let node = Node::Resource(index);
            let edges = parents.entry(node.clone()).or_default();
            for key in &resource.dependencies {
                edges.insert(
                    keys.get(key.as_str())
                        .ok_or_else(|| StateError::ResourceInvariant {
                            detail: format!("resource {:?} lost parent {key:?}", resource.key),
                        })?
                        .clone(),
                );
            }
            by_step.entry(&resource.step_id).or_default().push(node);
        }
        for (index, checkpoint) in checkpoints.iter().enumerate() {
            if !by_step.contains_key(checkpoint.step_id.as_str()) {
                let node = Node::Legacy(index);
                parents.entry(node.clone()).or_default();
                by_step.entry(&checkpoint.step_id).or_default().push(node);
            }
        }
        // Applying today's step edges to recorded resources merges different
        // generations and can invent cycles. Their saved parent keys are authoritative.
        for (step, nodes) in &by_step {
            for ancestor in plan.ancestors(step) {
                if let Some(prerequisites) = by_step.get(ancestor.as_str()) {
                    for child in nodes {
                        for parent in prerequisites {
                            let legacy_parent = matches!(parent, Node::Legacy(index)
                                if checkpoints[*index].resource_kind != ACTION_RESOURCE_KIND);
                            if matches!(child, Node::Legacy(_)) || legacy_parent {
                                parents
                                    .entry(child.clone())
                                    .or_default()
                                    .insert(parent.clone());
                            }
                        }
                    }
                }
            }
        }
        if let Some(context) = by_step.get(INSTANCE_RESOURCE_STEP) {
            for (step, nodes) in &by_step {
                if !step.is_empty() {
                    for node in nodes {
                        parents
                            .entry(node.clone())
                            .or_default()
                            .extend(context.iter().cloned());
                    }
                }
            }
        }
        let mut pending = parents.clone();
        let mut order = Vec::new();
        while !pending.is_empty() {
            let next = pending
                .iter()
                .find(|(_, edges)| edges.is_empty())
                .map(|(node, _)| node.clone())
                .ok_or_else(|| StateError::ResourceInvariant {
                    detail: "resource dependencies contain a cycle; teardown order is unknown"
                        .into(),
                })?;
            pending.remove(&next);
            for edges in pending.values_mut() {
                edges.remove(&next);
            }
            order.push(next);
        }
        order.reverse();
        Ok(Self { order, parents })
    }
}
