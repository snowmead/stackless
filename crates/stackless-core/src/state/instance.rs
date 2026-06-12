//! Instance records: one name, one truth (invariant 1).

use std::collections::BTreeMap;

use super::error::StateError;
use super::store::{Row, Store};

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

const SELECT_COLUMNS: &str = "name, substrate, status, definition, source_overrides, created_at, tombstoned_at, definition_dir";

impl Store {
    /// Create an instance record. Names are unique across substrates:
    /// a clash is an error naming the existing substrate, not a sibling.
    /// The UNIQUE PRIMARY KEY enforces this on both backends — fleet-wide
    /// when the remote plane is configured.
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
        let result = self.execute(
            "INSERT INTO instances (name, substrate, status, definition, source_overrides, created_at, definition_dir)
             VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6)",
            &[
                name.into(),
                substrate.into(),
                definition.into(),
                overrides_json.into(),
                Self::now().into(),
                definition_dir.into(),
            ],
        );
        match result {
            Ok(_) => self
                .instance(name)?
                .ok_or_else(|| StateError::InstanceNotFound { name: name.into() }),
            Err(err) => {
                // A failed insert under an existing name is the
                // uniqueness conflict (driver-agnostic: the PRIMARY KEY
                // rejected it on either backend). Distinguish it from a
                // real error by re-reading.
                if let Some(existing) = self.instance(name)? {
                    Err(StateError::InstanceExists {
                        name: name.into(),
                        existing_substrate: existing.substrate,
                    })
                } else {
                    Err(err)
                }
            }
        }
    }

    pub fn instance(&self, name: &str) -> Result<Option<InstanceRecord>, StateError> {
        self.query_row(
            &format!("SELECT {SELECT_COLUMNS} FROM instances WHERE name = ?1"),
            &[name.into()],
            row_to_instance,
        )
    }

    pub fn instances(&self) -> Result<Vec<InstanceRecord>, StateError> {
        self.query_map(
            &format!("SELECT {SELECT_COLUMNS} FROM instances ORDER BY name"),
            &[],
            row_to_instance,
        )
    }

    /// Teardown leaves a tombstone, not amnesia (§2).
    pub fn tombstone_instance(&self, name: &str) -> Result<(), StateError> {
        let changed = self.execute(
            "UPDATE instances SET status = 'tombstoned', tombstoned_at = ?2 WHERE name = ?1",
            &[name.into(), Self::now().into()],
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
        let changed = self.execute(
            "UPDATE instances SET status = 'active', definition = ?2, source_overrides = ?3,
             created_at = ?4, tombstoned_at = NULL WHERE name = ?1",
            &[
                name.into(),
                definition.into(),
                overrides_json.into(),
                Self::now().into(),
            ],
        )?;
        if changed == 0 {
            return Err(StateError::InstanceNotFound { name: name.into() });
        }
        self.execute(
            "DELETE FROM checkpoints WHERE instance = ?1",
            &[name.into()],
        )?;
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
        self.execute(
            "UPDATE instances SET source_overrides = ?2 WHERE name = ?1",
            &[name.into(), overrides_json.into()],
        )?;
        Ok(())
    }
}

fn row_to_instance(row: &Row) -> Result<InstanceRecord, StateError> {
    let status = row.get_string(2)?;
    let overrides_json = row.get_string(4)?;
    Ok(InstanceRecord {
        name: row.get_string(0)?,
        substrate: row.get_string(1)?,
        status: InstanceStatus::from_sql(&status),
        definition: row.get_string(3)?,
        source_overrides: serde_json::from_str(&overrides_json).unwrap_or_default(),
        created_at: row.get_i64(5)?,
        tombstoned_at: row.get_opt_i64(6)?,
        definition_dir: row.get_string(7)?,
    })
}
