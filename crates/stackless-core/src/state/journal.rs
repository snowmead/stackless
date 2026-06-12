//! The per-step checkpoint journal (§2): every step that creates a
//! resource records it before moving on.

use super::error::StateError;
use super::store::{Row, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub instance: String,
    pub step_id: String,
    pub resource_kind: String,
    pub resource_id: String,
    /// Step-specific JSON the substrate needs to re-find the resource.
    pub payload: String,
    pub recorded_at: i64,
}

impl Store {
    pub fn record_checkpoint(
        &self,
        instance: &str,
        step_id: &str,
        resource_kind: &str,
        resource_id: &str,
        payload: &str,
    ) -> Result<(), StateError> {
        self.execute(
            "INSERT INTO checkpoints (instance, step_id, resource_kind, resource_id, payload, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(instance, step_id) DO UPDATE SET
               resource_kind = excluded.resource_kind,
               resource_id = excluded.resource_id,
               payload = excluded.payload,
               recorded_at = excluded.recorded_at",
            &[
                instance.into(),
                step_id.into(),
                resource_kind.into(),
                resource_id.into(),
                payload.into(),
                Self::now().into(),
            ],
        )?;
        Ok(())
    }

    pub fn checkpoint(
        &self,
        instance: &str,
        step_id: &str,
    ) -> Result<Option<Checkpoint>, StateError> {
        self.query_row(
            "SELECT instance, step_id, resource_kind, resource_id, payload, recorded_at
             FROM checkpoints WHERE instance = ?1 AND step_id = ?2",
            &[instance.into(), step_id.into()],
            Row::decode_checkpoint,
        )
    }

    /// All checkpoints for an instance in recording order — what an
    /// interrupted `down` must hunt down.
    pub fn checkpoints(&self, instance: &str) -> Result<Vec<Checkpoint>, StateError> {
        self.query_map(
            "SELECT instance, step_id, resource_kind, resource_id, payload, recorded_at
             FROM checkpoints WHERE instance = ?1 ORDER BY rowid",
            &[instance.into()],
            Row::decode_checkpoint,
        )
    }

    /// Remove one checkpoint after its resource is verifiably gone.
    pub fn remove_checkpoint(&self, instance: &str, step_id: &str) -> Result<(), StateError> {
        self.execute(
            "DELETE FROM checkpoints WHERE instance = ?1 AND step_id = ?2",
            &[instance.into(), step_id.into()],
        )?;
        Ok(())
    }
}


