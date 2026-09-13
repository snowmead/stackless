//! A resource lifetime keeps its hosting provider until teardown proves absence.

use std::collections::BTreeMap;

use super::{ResourcePhase, StateError, Store};
use crate::def::placement::placement_key;

impl Store {
    pub fn placements(&self, owner: &str) -> Result<BTreeMap<String, String>, StateError> {
        self.query_map(
            "SELECT node, provider FROM placements WHERE owner_id = ?1 ORDER BY node",
            &[owner.into()],
            |row| Ok((row.get_string(0)?, row.get_string(1)?)),
        )
        .map(|rows| rows.into_iter().collect())
    }

    /// Call while holding the instance operation claim, before external effects.
    /// Retired bindings remain readable for old receipts and can change only
    /// after all checkpoints and unfinished resources for that node are gone.
    pub fn bind_placements(
        &self,
        instance: &str,
        desired: &BTreeMap<String, String>,
    ) -> Result<(), StateError> {
        let owner = self
            .instance(instance)?
            .ok_or_else(|| StateError::InstanceNotFound {
                name: instance.into(),
            })?;
        let recorded = self.placements(&owner.instance_id)?;
        let checkpoints = self.checkpoints(instance)?;
        let resources = self.resources(&owner.instance_id)?;
        for (node, provider) in desired {
            let existing = recorded.get(node);
            let has_evidence = checkpoints
                .iter()
                .any(|cp| placement_key(&cp.step_id).as_ref() == Some(node))
                || resources.iter().any(|resource| {
                    resource.phase != ResourcePhase::Absent
                        && placement_key(&resource.step_id).as_ref() == Some(node)
                });
            // A receipt created through the legacy state API without a binding
            // belongs to the instance's default provider. Never adopt it on a new target.
            let prior = existing
                .map(String::as_str)
                .unwrap_or(owner.substrate.as_str());
            if has_evidence && prior != provider {
                return Err(StateError::PlacementConflict {
                    instance: instance.into(),
                    node: node.clone(),
                    existing: prior.into(),
                    requested: provider.clone(),
                });
            }
            if provider.is_empty()
                || !(node.starts_with("service:") || node.starts_with("integration:"))
            {
                return Err(StateError::ResourceInvariant {
                    detail: "invalid placement binding".into(),
                });
            }
        }
        for (node, provider) in desired {
            self.execute("INSERT INTO placements (owner_id, node, provider) VALUES (?1, ?2, ?3) ON CONFLICT(owner_id, node) DO UPDATE SET provider = excluded.provider", &[owner.instance_id.as_str().into(), node.as_str().into(), provider.as_str().into()])?;
        }
        Ok(())
    }
}
