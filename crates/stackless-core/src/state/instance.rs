//! Instance records: one name, one truth (invariant 1).

use std::collections::BTreeMap;

use super::error::StateError;
use super::row::Row;
use super::store::Store;
use crate::types::DnsName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceStatus {
    Active,
    Tombstoned,
}

impl InstanceStatus {
    pub(super) fn from_sql(s: &str) -> Result<Self, StateError> {
        match s {
            "active" => Ok(Self::Active),
            "tombstoned" => Ok(Self::Tombstoned),
            other => Err(StateError::RowDecode {
                column: 2,
                detail: format!("unknown instance status {other:?}"),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub name: DnsName,
    /// Immutable for one birth; display names can be reused after teardown.
    pub instance_id: String,
    /// Provider namespace. Legacy instances retain their recorded name.
    pub resource_namespace: String,
    pub substrate: DnsName,
    pub status: InstanceStatus,
    /// The definition snapshot taken at creation (raw stackless.toml).
    pub definition: String,
    /// Recorded per-invocation `--source` pins (service → path).
    pub source_overrides: BTreeMap<String, String>,
    /// Whether `--dirty` was passed: snapshot source pins instead of using
    /// them in place.
    pub dirty: bool,
    /// The directory the definition file came from at creation; the
    /// sibling secrets env file resolves from here on resume.
    pub definition_dir: String,
    pub created_at: i64,
    pub tombstoned_at: Option<i64>,
}

const SELECT_COLUMNS: &str = "name, substrate, status, definition, source_overrides, created_at, tombstoned_at, definition_dir, dirty, instance_id, resource_namespace";

impl TryFrom<&Row> for InstanceRecord {
    type Error = StateError;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        let status = row.get_string(2)?;
        let overrides_json = row.get_string(4)?;
        let name = row.get_string(0)?;
        let substrate = row.get_string(1)?;
        Ok(Self {
            name: DnsName::try_new(&name).map_err(|err| StateError::RowDecode {
                column: 0,
                detail: err.to_string(),
            })?,
            substrate: DnsName::try_new(&substrate).map_err(|err| StateError::RowDecode {
                column: 1,
                detail: err.to_string(),
            })?,
            instance_id: row.get_string(9)?,
            resource_namespace: row.get_string(10)?,
            status: InstanceStatus::from_sql(&status)?,
            definition: row.get_string(3)?,
            source_overrides: serde_json::from_str(&overrides_json).map_err(|err| {
                StateError::RowDecode {
                    column: 4,
                    detail: err.to_string(),
                }
            })?,
            created_at: row.get_i64(5)?,
            tombstoned_at: row.get_opt_i64(6)?,
            definition_dir: row.get_string(7)?,
            dirty: row.get_i64(8)? != 0,
        })
    }
}

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
        dirty: bool,
    ) -> Result<InstanceRecord, StateError> {
        let overrides_json =
            serde_json::to_string(source_overrides).unwrap_or_else(|_| "{}".into());
        let instance_id = uuid::Uuid::new_v4().simple().to_string();
        let namespace = format!("sl-{instance_id}");
        let result = self.execute(
            "INSERT INTO instances (name, substrate, status, definition, source_overrides, created_at, definition_dir, dirty, instance_id, resource_namespace)
             VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            &[
                name.into(),
                substrate.into(),
                definition.into(),
                overrides_json.into(),
                Self::now().into(),
                definition_dir.into(),
                i64::from(dirty).into(),
                instance_id.into(),
                namespace.into(),
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
                        existing_substrate: existing.substrate.as_str().to_owned(),
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
            |row| InstanceRecord::try_from(row),
        )
    }

    pub fn instances(&self) -> Result<Vec<InstanceRecord>, StateError> {
        self.query_map(
            &format!("SELECT {SELECT_COLUMNS} FROM instances ORDER BY name"),
            &[],
            |row| InstanceRecord::try_from(row),
        )
    }

    /// Teardown leaves a tombstone, not amnesia (§2).
    pub fn tombstone_instance(&self, name: &str) -> Result<(), StateError> {
        let changed = self.execute(
            "UPDATE instances SET status = 'tombstoned', tombstoned_at = ?2 WHERE name = ?1
             AND NOT EXISTS (SELECT 1 FROM checkpoints WHERE instance = ?1)
             AND NOT EXISTS (SELECT 1 FROM resources WHERE owner_id = instances.instance_id AND phase != 'absent')",
            &[name.into(), Self::now().into()],
        )?;
        if changed == 0 {
            return Err(if self.instance(name)?.is_none() {
                StateError::InstanceNotFound { name: name.into() }
            } else {
                StateError::ResourceInvariant {
                    detail: format!(
                        "instance {name:?} still has teardown evidence or is not eligible for this transition"
                    ),
                }
            });
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
        dirty: bool,
    ) -> Result<(), StateError> {
        let overrides_json =
            serde_json::to_string(source_overrides).unwrap_or_else(|_| "{}".into());
        let instance_id = uuid::Uuid::new_v4().simple().to_string();
        let namespace = format!("sl-{instance_id}");
        let changed = self.execute(
            "UPDATE instances SET status = 'active', definition = ?2, source_overrides = ?3,
             created_at = ?4, tombstoned_at = NULL, dirty = ?5, instance_id = ?6, resource_namespace = ?7 WHERE name = ?1 AND status = 'tombstoned'
             AND NOT EXISTS (SELECT 1 FROM checkpoints WHERE instance = ?1)
             AND NOT EXISTS (SELECT 1 FROM resources WHERE owner_id = instances.instance_id AND phase != 'absent')",
            &[
                name.into(),
                definition.into(),
                overrides_json.into(),
                Self::now().into(),
                i64::from(dirty).into(),
                instance_id.into(),
                namespace.into(),
            ],
        )?;
        if changed == 0 {
            return Err(if self.instance(name)?.is_none() {
                StateError::InstanceNotFound { name: name.into() }
            } else {
                StateError::ResourceInvariant {
                    detail: format!(
                        "instance {name:?} still has teardown evidence or is not eligible for this transition"
                    ),
                }
            });
        }
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

    /// Record per-invocation dirty mode on resume.
    pub fn update_dirty(&self, name: &str, dirty: bool) -> Result<(), StateError> {
        self.execute(
            "UPDATE instances SET dirty = ?2 WHERE name = ?1",
            &[name.into(), i64::from(dirty).into()],
        )?;
        Ok(())
    }
}
