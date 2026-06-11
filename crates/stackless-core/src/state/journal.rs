//! The per-step checkpoint journal (§2): every step that creates a
//! resource records it before moving on.

use rusqlite::OptionalExtension;

use super::error::StateError;
use super::store::Store;

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
        self.conn.execute(
            "INSERT INTO checkpoints (instance, step_id, resource_kind, resource_id, payload, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(instance, step_id) DO UPDATE SET
               resource_kind = excluded.resource_kind,
               resource_id = excluded.resource_id,
               payload = excluded.payload,
               recorded_at = excluded.recorded_at",
            rusqlite::params![instance, step_id, resource_kind, resource_id, payload, Self::now()],
        )?;
        Ok(())
    }

    pub fn checkpoint(
        &self,
        instance: &str,
        step_id: &str,
    ) -> Result<Option<Checkpoint>, StateError> {
        self.conn
            .query_row(
                "SELECT instance, step_id, resource_kind, resource_id, payload, recorded_at
                 FROM checkpoints WHERE instance = ?1 AND step_id = ?2",
                [instance, step_id],
                row_to_checkpoint,
            )
            .optional()
            .map_err(Into::into)
    }

    /// All checkpoints for an instance in recording order — what an
    /// interrupted `down` must hunt down.
    pub fn checkpoints(&self, instance: &str) -> Result<Vec<Checkpoint>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT instance, step_id, resource_kind, resource_id, payload, recorded_at
             FROM checkpoints WHERE instance = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([instance], row_to_checkpoint)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Remove one checkpoint after its resource is verifiably gone.
    pub fn remove_checkpoint(&self, instance: &str, step_id: &str) -> Result<(), StateError> {
        self.conn.execute(
            "DELETE FROM checkpoints WHERE instance = ?1 AND step_id = ?2",
            [instance, step_id],
        )?;
        Ok(())
    }
}

fn row_to_checkpoint(row: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        instance: row.get(0)?,
        step_id: row.get(1)?,
        resource_kind: row.get(2)?,
        resource_id: row.get(3)?,
        payload: row.get(4)?,
        recorded_at: row.get(5)?,
    })
}
