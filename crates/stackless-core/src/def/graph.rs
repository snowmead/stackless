//! The derived dependency graph (ARCHITECTURE.md §1).
//!
//! Wiring is interpolation, and the graph is derived from it — never
//! declared separately. Two edge classes fall out of the namespace:
//!
//! - `${datastores.X.url}` is an **ordering** edge: the value does not
//!   exist until the datastore is provisioned, so the referencing
//!   service starts after it.
//! - `${services.X.origin}` is **wiring only**: origins are derivable
//!   from the instance name alone on every substrate, so mutual
//!   references (api ↔ web CORS) are recorded but never order startup —
//!   which is exactly why they are not cycles.
//!
//! Representation and traversal live in oxgraph (CSR over dense node
//! indices); the one algorithm it does not ship — Kahn's topological
//! sort with cycle detection — is implemented generically over its
//! topology capability traits. CSR is outgoing-only by design, so
//! in-degrees are computed in one successors pass rather than binding
//! `ElementPredecessors`.

use std::collections::{BTreeSet, VecDeque};

use oxgraph::csr::{CsrNativeGraph, CsrNodeId};
use oxgraph::topology::{DenseElementIndex, ElementSuccessors, TopologyCounts};
use serde::Serialize;

use super::error::DefError;
use super::interp::{self, Reference};
use super::model::StackDef;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Node {
    Datastore(String),
    Service(String),
}

impl Node {
    pub fn name(&self) -> &str {
        match self {
            Self::Datastore(name) | Self::Service(name) => name,
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
        // Dense indices: datastores first, then services, both in the
        // definition's (sorted) order — deterministic by construction.
        let nodes: Vec<Node> = def
            .datastores
            .keys()
            .map(|name| Node::Datastore(name.clone()))
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
                        Reference::StackName
                        | Reference::InstanceName
                        | Reference::Secret(_)
                        | Reference::IntegrationOutput { .. } => continue,
                    };
                    let Some(target_idx) = index_of(&target) else {
                        continue;
                    };
                    wiring.insert((service_idx, target_idx));
                    if matches!(target, Node::Datastore(_)) {
                        // Edge points dependency → dependent so Kahn
                        // emits dependencies first.
                        ordering_edges.insert((target_idx, service_idx));
                    }
                }
            }
        }

        let (offsets, targets) = to_csr(nodes.len(), &ordering_edges);
        let graph = CsrNativeGraph::<u32, u32>::validate(nodes.len() as u32, &offsets, &targets)
            .map_err(|err| DefError::WiringCycle {
                // Unreachable for edges we just built; surface honestly
                // rather than panic if oxgraph rejects the layout.
                nodes: format!("internal CSR layout error: {err:?}"),
            })?;
        let order = kahn_topological_order(&graph, (0..nodes.len() as u32).map(CsrNodeId::new))
            .map_err(|stuck| DefError::WiringCycle {
                nodes: stuck
                    .iter()
                    .map(|id| nodes[id.get() as usize].name().to_owned())
                    .collect::<Vec<_>>()
                    .join(" -> "),
            })?;

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

/// Sorted edge set → CSR offsets/targets (the layout oxgraph borrows).
fn to_csr(node_count: usize, edges: &BTreeSet<(u32, u32)>) -> (Vec<u32>, Vec<u32>) {
    let mut offsets = Vec::with_capacity(node_count + 1);
    let mut targets = Vec::with_capacity(edges.len());
    let mut edge_iter = edges.iter().peekable();
    offsets.push(0);
    for node in 0..node_count as u32 {
        while let Some((from, to)) = edge_iter.peek() {
            if *from != node {
                break;
            }
            targets.push(*to);
            edge_iter.next();
        }
        offsets.push(targets.len() as u32);
    }
    (offsets, targets)
}

/// Kahn's algorithm, generic over oxgraph's topology capability traits.
///
/// CSR views are outgoing-only (no `ElementPredecessors` by design), so
/// in-degrees come from one pass over successors. `elements` supplies
/// enumeration, which the capability traits deliberately do not.
///
/// Returns the topological order, or `Err` with the elements stuck in a
/// cycle.
pub fn kahn_topological_order<G>(
    graph: &G,
    elements: impl Iterator<Item = G::ElementId> + Clone,
) -> Result<Vec<G::ElementId>, Vec<G::ElementId>>
where
    G: ElementSuccessors + DenseElementIndex + TopologyCounts,
    G::ElementId: Copy,
{
    let mut indegree = vec![0usize; graph.element_bound()];
    for element in elements.clone() {
        for successor in graph.element_successors(element) {
            indegree[graph.element_index(successor)] += 1;
        }
    }
    let mut queue: VecDeque<G::ElementId> = elements
        .clone()
        .filter(|e| indegree[graph.element_index(*e)] == 0)
        .collect();
    let mut order = Vec::with_capacity(graph.element_count());
    while let Some(element) = queue.pop_front() {
        order.push(element);
        for successor in graph.element_successors(element) {
            let index = graph.element_index(successor);
            indegree[index] -= 1;
            if indegree[index] == 0 {
                queue.push_back(successor);
            }
        }
    }
    if order.len() == graph.element_count() {
        Ok(order)
    } else {
        Err(elements
            .filter(|e| indegree[graph.element_index(*e)] > 0)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csr(node_count: usize, edges: &[(u32, u32)]) -> (Vec<u32>, Vec<u32>) {
        to_csr(node_count, &edges.iter().copied().collect())
    }

    #[test]
    fn kahn_orders_a_dag() {
        // 0 -> 1 -> 3, 0 -> 2 -> 3
        let (offsets, targets) = csr(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let graph = CsrNativeGraph::<u32, u32>::validate(4, &offsets, &targets).unwrap();
        let order: Vec<u32> = kahn_topological_order(&graph, (0..4).map(CsrNodeId::new))
            .unwrap()
            .into_iter()
            .map(CsrNodeId::get)
            .collect();
        assert_eq!(order, vec![0, 1, 2, 3]);
    }

    #[test]
    fn kahn_reports_the_cycle_members() {
        // 0 -> 1 -> 2 -> 1, 3 isolated
        let (offsets, targets) = csr(4, &[(0, 1), (1, 2), (2, 1)]);
        let graph = CsrNativeGraph::<u32, u32>::validate(4, &offsets, &targets).unwrap();
        let stuck: Vec<u32> = kahn_topological_order(&graph, (0..4).map(CsrNodeId::new))
            .unwrap_err()
            .into_iter()
            .map(CsrNodeId::get)
            .collect();
        assert_eq!(stuck, vec![1, 2]);
    }

    #[test]
    fn kahn_handles_no_edges() {
        let (offsets, targets) = csr(3, &[]);
        let graph = CsrNativeGraph::<u32, u32>::validate(3, &offsets, &targets).unwrap();
        let order = kahn_topological_order(&graph, (0..3).map(CsrNodeId::new)).unwrap();
        assert_eq!(order.len(), 3);
    }
}
