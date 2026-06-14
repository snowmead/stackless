//! The derived dependency graph (ARCHITECTURE.md §1).
//!
//! Wiring is interpolation, and the graph is derived from it — never
//! declared separately. Two edge classes fall out of the namespace:
//!
//! - `${datastores.X.url}` is an **ordering** edge: the value does not
//!   exist until the datastore is provisioned, so the referencing
//!   service starts after it.
//! - `${integrations.X.output}` is an **ordering** edge: outputs exist
//!   only after the integration is provisioned.
//! - `${services.X.origin}` is **wiring only**: origins are derivable
//!   from the instance name alone on every substrate, so mutual
//!   references (api ↔ web CORS) are recorded but never order startup —
//!   which is exactly why they are not cycles.
//!
//! Representation, ordering, and cycle detection all live in oxgraph: the
//! nodes and ordering edges feed a `GraphBuilder`, `topological_sort` yields
//! the startup order, and on the rare cycle `strongly_connected_components`
//! names the members for the error.

use std::collections::BTreeSet;

use oxgraph::algo::{strongly_connected_components, topological_sort};
use oxgraph::graph_build::{GraphBuildError, GraphBuilder};
use serde::Serialize;

use super::error::DefError;
use super::interp::{self, Reference};
use super::model::StackDef;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Node {
    Integration(String),
    Datastore(String),
    Service(String),
}

impl Node {
    pub fn name(&self) -> &str {
        match self {
            Self::Integration(name) | Self::Datastore(name) | Self::Service(name) => name,
        }
    }
}

/// The graph derived from one validated definition.
#[derive(Debug, Serialize)]
pub struct DependencyGraph {
    nodes: Vec<Node>,
    /// Dependencies before dependents; also the `prepare` order.
    startup_order: Vec<Node>,
    /// Every reference edge (referencing service → target), origins
    /// included — the seam later phases derive egress policy from.
    wiring: Vec<(Node, Node)>,
}

impl DependencyGraph {
    /// Derive edges from the definition's interpolation references.
    ///
    /// Call on a validated definition: undeclared references have
    /// already been rejected, so lookups here cannot miss.
    pub fn derive(def: &StackDef) -> Result<Self, DefError> {
        // Dense indices: integrations, then datastores, then services —
        // all in the definition's (sorted) order for deterministic ties.
        let nodes: Vec<Node> = def
            .integrations
            .keys()
            .map(|name| Node::Integration(name.clone()))
            .chain(
                def.datastores
                    .keys()
                    .map(|name| Node::Datastore(name.clone())),
            )
            .chain(def.services.keys().map(|name| Node::Service(name.clone())))
            .collect();
        let index_of =
            |node: &Node| -> Option<u32> { nodes.iter().position(|n| n == node).map(|i| i as u32) };

        let mut ordering_edges: BTreeSet<(u32, u32)> = BTreeSet::new();
        let mut wiring: BTreeSet<(u32, u32)> = BTreeSet::new();
        for (service_name, service) in &def.services {
            let service_node = Node::Service(service_name.clone());
            let Some(service_idx) = index_of(&service_node) else {
                continue;
            };
            let mut values: Vec<(String, String)> = service
                .env
                .iter()
                .map(|(k, v)| (format!("services.{service_name}.env.{k}"), v.clone()))
                .collect();
            for substrate in service.substrates.keys() {
                for (k, v) in service.substrate_env(service_name, substrate)? {
                    values.push((format!("services.{service_name}.{substrate}.env.{k}"), v));
                }
            }
            for (location, value) in &values {
                for reference in interp::references(value, location)? {
                    let target = match reference {
                        Reference::DatastoreUrl(name) => Node::Datastore(name),
                        Reference::ServiceOrigin(name) => Node::Service(name),
                        Reference::IntegrationOutput { integration, .. } => {
                            Node::Integration(integration)
                        }
                        Reference::StackName | Reference::InstanceName | Reference::Secret(_) => {
                            continue;
                        }
                    };
                    let Some(target_idx) = index_of(&target) else {
                        continue;
                    };
                    wiring.insert((service_idx, target_idx));
                    if matches!(target, Node::Datastore(_) | Node::Integration(_)) {
                        // Edge points dependency → dependent so Kahn
                        // emits dependencies first.
                        ordering_edges.insert((target_idx, service_idx));
                    }
                }
            }
        }

        if let Some(verify) = &def.stack.verify {
            for (key, value) in &verify.env {
                let location = format!("stack.verify.env.{key}");
                for reference in interp::references(value, &location)? {
                    let Reference::IntegrationOutput { integration, .. } = reference else {
                        continue;
                    };
                    let target = Node::Integration(integration);
                    let Some(target_idx) = index_of(&target) else {
                        continue;
                    };
                    // Verify runs after `up`; record wiring only.
                    let _ = target_idx;
                }
            }
        }

        let mut builder = GraphBuilder::<u32, u32>::new();
        let mut node_ids = Vec::with_capacity(nodes.len());
        for _ in &nodes {
            node_ids.push(builder.add_node().map_err(internal_layout_error)?);
        }
        for &(from, to) in &ordering_edges {
            builder
                .add_edge(node_ids[from as usize], node_ids[to as usize])
                .map_err(internal_layout_error)?;
        }
        let graph = builder.freeze().map_err(internal_layout_error)?;

        let order = match topological_sort(&graph, &node_ids) {
            Ok(order) => order,
            // Toposort only flags that no order exists; SCC names the cycle
            // members so the error points at the wiring that closed it.
            Err(_) => {
                let cycle = strongly_connected_components(&graph, &node_ids)
                    .into_iter()
                    .filter(|component| component.len() > 1)
                    .flatten()
                    .map(|id| nodes[id.get() as usize].name().to_owned())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(DefError::WiringCycle { nodes: cycle });
            }
        };

        let startup_order = order
            .into_iter()
            .map(|id| nodes[id.get() as usize].clone())
            .collect();
        let wiring = wiring
            .into_iter()
            .map(|(from, to)| (nodes[from as usize].clone(), nodes[to as usize].clone()))
            .collect();
        Ok(Self {
            nodes,
            startup_order,
            wiring,
        })
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn startup_order(&self) -> &[Node] {
        &self.startup_order
    }

    pub fn wiring(&self) -> &[(Node, Node)] {
        &self.wiring
    }
}

/// Builder/freeze failures are unreachable for the dense edge set we just
/// built; surface honestly rather than panic if oxgraph rejects the layout.
fn internal_layout_error(err: GraphBuildError<u32, u32>) -> DefError {
    DefError::WiringCycle {
        nodes: format!("internal graph layout error: {err:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxgraph::algo::ToposortError;
    use oxgraph::graph_build::{FrozenGraph, GraphNodeId};

    fn build(
        node_count: u32,
        edges: &[(u32, u32)],
    ) -> (FrozenGraph<u32, u32>, Vec<GraphNodeId<u32>>) {
        let mut builder = GraphBuilder::<u32, u32>::new();
        let ids: Vec<_> = (0..node_count)
            .map(|_| builder.add_node().unwrap())
            .collect();
        for &(from, to) in edges {
            builder
                .add_edge(ids[from as usize], ids[to as usize])
                .unwrap();
        }
        (builder.freeze().unwrap(), ids)
    }

    #[test]
    fn topological_sort_orders_a_dag() {
        // 0 -> 1 -> 3, 0 -> 2 -> 3
        let (graph, ids) = build(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let order: Vec<u32> = topological_sort(&graph, &ids)
            .unwrap()
            .into_iter()
            .map(|id| id.get())
            .collect();
        assert_eq!(order, vec![0, 1, 2, 3]);
    }

    #[test]
    fn scc_reports_the_cycle_members() {
        // 0 -> 1 -> 2 -> 1, 3 isolated
        let (graph, ids) = build(4, &[(0, 1), (1, 2), (2, 1)]);
        assert_eq!(
            topological_sort(&graph, &ids).unwrap_err(),
            ToposortError::Cycle
        );
        let mut cycle: Vec<u32> = strongly_connected_components(&graph, &ids)
            .into_iter()
            .filter(|component| component.len() > 1)
            .flatten()
            .map(|id| id.get())
            .collect();
        cycle.sort_unstable();
        assert_eq!(cycle, vec![1, 2]);
    }

    #[test]
    fn topological_sort_handles_no_edges() {
        let (graph, ids) = build(3, &[]);
        assert_eq!(topological_sort(&graph, &ids).unwrap().len(), 3);
    }
}
