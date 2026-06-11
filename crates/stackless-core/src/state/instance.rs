//! Instance records: one name, one truth (invariant 1).

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;

use super::error::StateError;
use super::store::Store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceStatus {
    Active,
    Tombstoned,
}

impl InstanceStatus {
    fn from_sql(s: &str) -> Self {
        match s {
            "tombstoned" => Self::Tombstoned,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub name: String,
    pub substrate: String,
    pub status: InstanceStatus,
    /// The definition snapshot taken at creation (raw stackless.toml).
    pub definition: String,
    /// Recorded per-invocation `--source` pins (service → path).
    pub source_overrides: BTreeMap<String, String>,
    /// The directory the definition file came from at creation; the
    /// sibling secrets env file resolves from here on resume.
    pub definition_dir: String,
    pub created_at: i64,
    pub tombstoned_at: Option<i64>,
}

impl Store {
    /// Create an instance record. Names are unique across substrates:
    /// a clash is an error naming the existing substrate, not a sibling.
    pub fn create_instance(
        &self,
        name: &str,
        substrate: &str,
        definition: &str,
        source_overrides: &BTreeMap<String, String>,
        definition_dir: &str,
    ) -> Result<InstanceRecord, StateError> {
        let overrides_json =
            serde_json::to_string(source_overrides).unwrap_or_else(|_| "{}".into());
        let result = self.conn.execute(
            "INSERT INTO instances (name, substrate, status, definition, source_overrides, created_at, definition_dir)
             VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6)",
            rusqlite::params![name, substrate, definition, overrides_json, Self::now(), definition_dir],
        );
        match result {
            Ok(_) => self
                .instance(name)?
                .ok_or_else(|| StateError::InstanceNotFound { name: name.into() }),
            Err(err) if is_unique_violation(&err) => {
                let existing = self
                    .instance(name)?
                    .map(|r| r.substrate)
                    .unwrap_or_default();
                Err(StateError::InstanceExists {
                    name: name.into(),
                    existing_substrate: existing,
                })
            }
            Err(err) => Err(err.into()),
        }
    }

    pub fn instance(&self, name: &str) -> Result<Option<InstanceRecord>, StateError> {
        self.conn
            .query_row(
                "SELECT name, substrate, status, definition, source_overrides, created_at, tombstoned_at, definition_dir
                 FROM instances WHERE name = ?1",
                [name],
                row_to_instance,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn instances(&self) -> Result<Vec<InstanceRecord>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, substrate, status, definition, source_overrides, created_at, tombstoned_at, definition_dir
             FROM instances ORDER BY name",
        )?;
        let rows = stmt.query_map([], row_to_instance)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Teardown leaves a tombstone, not amnesia (§2).
    pub fn tombstone_instance(&self, name: &str) -> Result<(), StateError> {
        let changed = self.conn.execute(
            "UPDATE instances SET status = 'tombstoned', tombstoned_at = ?2 WHERE name = ?1",
            rusqlite::params![name, Self::now()],
        )?;
        if changed == 0 {
            return Err(StateError::InstanceNotFound { name: name.into() });
        }
        Ok(())
    }

    /// `up` on a tombstone is a fresh birth under the old name: new
    /// definition snapshot, active status, empty journal.
    pub fn revive_instance(
        &self,
        name: &str,
        definition: &str,
        source_overrides: &BTreeMap<String, String>,
    ) -> Result<(), StateError> {
        let overrides_json =
            serde_json::to_string(source_overrides).unwrap_or_else(|_| "{}".into());
        let changed = self.conn.execute(
            "UPDATE instances SET status = 'active', definition = ?2, source_overrides = ?3,
             created_at = ?4, tombstoned_at = NULL WHERE name = ?1",
            rusqlite::params![name, definition, overrides_json, Self::now()],
        )?;
        if changed == 0 {
            return Err(StateError::InstanceNotFound { name: name.into() });
        }
        self.conn
            .execute("DELETE FROM checkpoints WHERE instance = ?1", [name])?;
        Ok(())
    }

    /// Record per-invocation overrides on resume (an explicit, recorded
    /// choice — never ambient discovery).
    pub fn update_source_overrides(
        &self,
        name: &str,
        source_overrides: &BTreeMap<String, String>,
    ) -> Result<(), StateError> {
        let overrides_json =
            serde_json::to_string(source_overrides).unwrap_or_else(|_| "{}".into());
        self.conn.execute(
            "UPDATE instances SET source_overrides = ?2 WHERE name = ?1",
            rusqlite::params![name, overrides_json],
        )?;
        Ok(())
    }
}

fn row_to_instance(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstanceRecord> {
    let status: String = row.get(2)?;
    let overrides_json: String = row.get(4)?;
    Ok(InstanceRecord {
        name: row.get(0)?,
        substrate: row.get(1)?,
        status: InstanceStatus::from_sql(&status),
        definition: row.get(3)?,
        source_overrides: serde_json::from_str(&overrides_json).unwrap_or_default(),
        created_at: row.get(5)?,
        tombstoned_at: row.get(6)?,
        definition_dir: row.get(7)?,
    })
}

fn is_unique_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}
