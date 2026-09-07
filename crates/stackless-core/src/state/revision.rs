//! Desired input and completed reconciliation are separate durable facts.

use serde::{Deserialize, Serialize};

use super::{StateError, Store};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionState {
    pub desired_revision: String,
    pub applied_revision: Option<String>,
}

impl Store {
    pub fn stage_definition(
        &self,
        instance: &str,
        definition: &str,
        revision: &str,
    ) -> Result<(), StateError> {
        let record = self
            .instance(instance)?
            .ok_or_else(|| StateError::InstanceNotFound {
                name: instance.into(),
            })?;
        let previous = self.query_row(
            "SELECT definition FROM definition_revisions WHERE owner_id = ?1 AND revision = ?2",
            &[record.instance_id.as_str().into(), revision.into()],
            |row| row.get_string(0),
        )?;
        // The digest covers parsed inputs. Comments can change without changing the revision.
        let snapshot = previous
            .as_deref()
            .filter(|previous| {
                match (
                    crate::def::StackDef::parse_snapshot(previous),
                    crate::def::StackDef::parse_snapshot(definition),
                ) {
                    (Ok(old), Ok(new)) => serde_json::to_value(old)
                        .ok()
                        .zip(serde_json::to_value(new).ok())
                        .is_some_and(|(old, new)| old == new),
                    _ => *previous == definition,
                }
            })
            .unwrap_or(definition);
        self.execute_atomic(&[
            ("INSERT INTO definition_revisions(owner_id, revision, definition, recorded_at) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(owner_id, revision) DO UPDATE SET recorded_at = excluded.recorded_at
                WHERE definition_revisions.definition = excluded.definition",
                vec![record.instance_id.as_str().into(), revision.into(), snapshot.into(), Self::now().into()]),
            ("INSERT INTO instance_revisions(owner_id, desired_revision) VALUES (?1, ?2)
                ON CONFLICT(owner_id) DO UPDATE SET desired_revision = excluded.desired_revision",
                vec![record.instance_id.as_str().into(), revision.into()]),
            ("UPDATE instances SET definition = ?2 WHERE instance_id = ?1 AND status = 'active'",
                vec![record.instance_id.as_str().into(), definition.into()]),
        ])
    }

    pub fn applied_definition(&self, owner: &str) -> Result<Option<String>, StateError> {
        self.query_row("SELECT d.definition FROM definition_revisions d JOIN instance_revisions i ON i.owner_id = d.owner_id AND i.applied_revision = d.revision WHERE d.owner_id = ?1", &[owner.into()], |row| row.get_string(0))
    }

    pub fn instance_revision(&self, owner: &str) -> Result<Option<RevisionState>, StateError> {
        self.query_row(
            "SELECT desired_revision, applied_revision FROM instance_revisions WHERE owner_id = ?1",
            &[owner.into()],
            |row| {
                Ok(RevisionState {
                    desired_revision: row.get_string(0)?,
                    applied_revision: row.get_opt_string(1)?,
                })
            },
        )
    }

    pub fn stage_step(
        &self,
        owner: &str,
        step: &str,
        revision: &str,
    ) -> Result<RevisionState, StateError> {
        self.execute("INSERT INTO step_revisions(owner_id, step_id, desired_revision) VALUES (?1, ?2, ?3) ON CONFLICT(owner_id, step_id) DO UPDATE SET desired_revision = excluded.desired_revision", &[owner.into(), step.into(), revision.into()])?;
        self.step_revision(owner, step)?
            .ok_or_else(|| StateError::ResourceInvariant {
                detail: "desired step revision missing after write".into(),
            })
    }

    pub fn step_revision(
        &self,
        owner: &str,
        step: &str,
    ) -> Result<Option<RevisionState>, StateError> {
        self.query_row("SELECT desired_revision, applied_revision FROM step_revisions WHERE owner_id = ?1 AND step_id = ?2", &[owner.into(), step.into()], |row| Ok(RevisionState { desired_revision: row.get_string(0)?, applied_revision: row.get_opt_string(1)? }))
    }

    pub fn step_applied(&self, owner: &str, step: &str, revision: &str) -> Result<(), StateError> {
        if self.execute("UPDATE step_revisions SET applied_revision = ?3 WHERE owner_id = ?1 AND step_id = ?2 AND desired_revision = ?3", &[owner.into(), step.into(), revision.into()])? != 1 {
            return Err(StateError::ResourceInvariant { detail: "completed step differs from desired revision".into() });
        }
        Ok(())
    }

    pub fn revision_applied(&self, owner: &str, revision: &str) -> Result<(), StateError> {
        if self.execute("UPDATE instance_revisions SET applied_revision = ?2 WHERE owner_id = ?1 AND desired_revision = ?2", &[owner.into(), revision.into()])? != 1 {
            return Err(StateError::ResourceInvariant { detail: "completed instance differs from desired revision".into() });
        }
        Ok(())
    }

    pub fn record_observation(
        &self,
        owner: &str,
        node: &str,
        evidence: &serde_json::Value,
    ) -> Result<(), StateError> {
        self.execute("INSERT INTO observations(owner_id, node, evidence_json, observed_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(owner_id, node) DO UPDATE SET evidence_json = excluded.evidence_json, observed_at = excluded.observed_at", &[owner.into(), node.into(), evidence.to_string().into(), Self::now().into()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn seed(store: &Store) -> String {
        let instance = store
            .create_instance("demo", "local", "original", &BTreeMap::new(), "", false)
            .unwrap();
        store
            .stage_definition("demo", "original", "revision-one")
            .unwrap();
        instance.instance_id
    }

    #[test]
    fn definition_and_revision_roll_back_together_when_the_final_write_fails() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.db")).unwrap();
        let owner = seed(&store);
        store.conn_for_tests().execute_batch("CREATE TRIGGER reject_definition BEFORE UPDATE OF definition ON instances BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
        assert!(
            store
                .stage_definition("demo", "changed", "revision-two")
                .is_err()
        );
        assert_eq!(
            store.instance("demo").unwrap().unwrap().definition,
            "original"
        );
        assert_eq!(
            store
                .instance_revision(&owner)
                .unwrap()
                .unwrap()
                .desired_revision,
            "revision-one"
        );
        let count: i64 = store
            .conn_for_tests()
            .query_row(
                "SELECT count(*) FROM definition_revisions WHERE revision = 'revision-two'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        store
            .conn_for_tests()
            .execute_batch("DROP TRIGGER reject_definition")
            .unwrap();
        store
            .stage_definition("demo", "changed", "revision-two")
            .unwrap();
        assert_eq!(
            store.instance("demo").unwrap().unwrap().definition,
            "changed"
        );
    }

    #[test]
    fn local_rejects_revision_collisions_and_rolls_back_inactive_owner_updates() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.db")).unwrap();
        {
            let owner = seed(&store);
            assert!(
                store
                    .stage_definition("demo", "different content", "revision-one")
                    .is_err()
            );
            assert_eq!(
                store.instance("demo").unwrap().unwrap().definition,
                "original"
            );
            store.tombstone_instance("demo").unwrap();
            assert!(
                store
                    .stage_definition("demo", "changed", "revision-two")
                    .is_err()
            );
            assert_eq!(
                store
                    .instance_revision(&owner)
                    .unwrap()
                    .unwrap()
                    .desired_revision,
                "revision-one"
            );
            let count = store
                .query_row(
                    "SELECT count(*) FROM definition_revisions WHERE revision = 'revision-two'",
                    &[],
                    |row| row.get_i64(0),
                )
                .unwrap();
            assert_eq!(count, Some(0));
        }
    }

    #[test]
    fn comments_can_change_without_replacing_the_immutable_semantic_revision() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.db")).unwrap();
        let original = "[stack]\nname='demo'\n[jobs.test]\nimage='alpine'";
        let owner = store
            .create_instance("demo", "local", original, &BTreeMap::new(), "", false)
            .unwrap();
        store
            .stage_definition("demo", original, "same-inputs")
            .unwrap();
        let commented = format!("# Changed comment\n{original}");
        store
            .stage_definition("demo", &commented, "same-inputs")
            .unwrap();
        assert_eq!(
            store.instance("demo").unwrap().unwrap().definition,
            commented
        );
        let saved = store
            .query_row(
                "SELECT definition FROM definition_revisions WHERE owner_id = ?1",
                &[owner.instance_id.into()],
                |row| row.get_string(0),
            )
            .unwrap();
        assert_eq!(saved.as_deref(), Some(original));
    }
}
