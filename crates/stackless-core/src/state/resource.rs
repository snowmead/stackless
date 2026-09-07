//! Durable resource inventory. Intent precedes creation; absence ends ownership.

use serde::{Deserialize, Serialize};

use super::row::Row;
use super::{Checkpoint, StateError, Store};

/// Instance-scoped resources outlive every step resource.
pub const INSTANCE_RESOURCE_STEP: &str = "";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Owned,
    Borrowed,
    Shared,
}

impl Ownership {
    fn as_str(self) -> &'static str {
        match self {
            Self::Owned => "owned",
            Self::Borrowed => "borrowed",
            Self::Shared => "shared",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePhase {
    Intent,
    Created,
    Ready,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRecord {
    pub owner_id: String,
    pub key: String,
    pub step_id: String,
    pub provider: String,
    pub ownership: Ownership,
    pub phase: ResourcePhase,
    pub resource_kind: String,
    pub resource_id: String,
    /// Provider recovery context. Contains credentials; never a public result.
    #[serde(skip_serializing)]
    pub payload: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub dependencies: Vec<String>,
}

impl ResourceRecord {
    pub fn checkpoint(&self, instance: &str) -> Checkpoint {
        Checkpoint {
            instance: instance.into(),
            step_id: self.step_id.clone(),
            resource_kind: self.resource_kind.clone(),
            resource_id: self.resource_id.clone(),
            payload: self.payload.clone(),
            recorded_at: self.updated_at,
        }
    }

    fn from_row(row: &Row) -> Result<Self, StateError> {
        let ownership = match row.get_string(4)?.as_str() {
            "owned" => Ownership::Owned,
            "borrowed" => Ownership::Borrowed,
            "shared" => Ownership::Shared,
            other => {
                return Err(StateError::ResourceInvariant {
                    detail: format!("unknown ownership {other:?}"),
                });
            }
        };
        let phase = match row.get_string(5)?.as_str() {
            "intent" => ResourcePhase::Intent,
            "created" => ResourcePhase::Created,
            "ready" => ResourcePhase::Ready,
            "absent" => ResourcePhase::Absent,
            other => {
                return Err(StateError::ResourceInvariant {
                    detail: format!("unknown resource phase {other:?}"),
                });
            }
        };
        Ok(Self {
            owner_id: row.get_string(0)?,
            key: row.get_string(1)?,
            step_id: row.get_string(2)?,
            provider: row.get_string(3)?,
            ownership,
            phase,
            resource_kind: row.get_string(6)?,
            resource_id: row.get_string(7)?,
            payload: row.get_string(8)?,
            created_at: row.get_i64(9)?,
            updated_at: row.get_i64(10)?,
            dependencies: serde_json::from_str(&row.get_string(11)?).map_err(|_| {
                StateError::ResourceInvariant {
                    detail: "unreadable resource dependencies".into(),
                }
            })?,
        })
    }
}

#[derive(Debug)]
pub struct ResourceIntent<'a> {
    pub owner_id: &'a str,
    pub key: &'a str,
    pub step_id: &'a str,
    pub provider: &'a str,
    pub ownership: Ownership,
    pub resource_kind: &'a str,
    /// Exact provider lookup key or idempotency key, known before creation.
    pub resource_id: &'a str,
    pub payload: &'a str,
    pub dependencies: &'a [&'a str],
}

impl Store {
    /// Persist before creating anything. A retry returns the original recovery
    /// context, so a changed request cannot hide an unfinished creation.
    pub fn resource_intent(
        &self,
        intent: ResourceIntent<'_>,
    ) -> Result<ResourceRecord, StateError> {
        for parent in intent.dependencies {
            if *parent == intent.key || self.resource(intent.owner_id, parent)?.is_none() {
                return Err(StateError::ResourceInvariant {
                    detail: format!(
                        "resource {:?} requires a prior parent intent {:?}",
                        intent.key, parent
                    ),
                });
            }
        }
        self.execute(
            "INSERT INTO resources (owner_id, resource_key, step_id, provider, ownership, phase,
             resource_kind, resource_id, payload, created_at, updated_at, dependencies)
             SELECT ?1, ?2, ?3, ?4, ?5, 'intent', ?6, ?7, ?8, ?9, ?9, ?10
             WHERE EXISTS (SELECT 1 FROM instances WHERE instance_id = ?1 AND status = 'active')
             ON CONFLICT(owner_id, resource_key) DO NOTHING",
            &[
                intent.owner_id.into(),
                intent.key.into(),
                intent.step_id.into(),
                intent.provider.into(),
                intent.ownership.as_str().into(),
                intent.resource_kind.into(),
                intent.resource_id.into(),
                intent.payload.into(),
                Self::now().into(),
                serde_json::to_string(intent.dependencies)
                    .map_err(|_| StateError::ResourceInvariant {
                        detail: "cannot encode resource dependencies".into(),
                    })?
                    .into(),
            ],
        )?;
        let record = self.resource(intent.owner_id, intent.key)?.ok_or_else(|| {
            StateError::ResourceInvariant {
                detail: format!("owner {} is not active", intent.owner_id),
            }
        })?;
        if record.dependencies != intent.dependencies
            || record.provider != intent.provider
            || record.ownership != intent.ownership
            || record.step_id != intent.step_id
            || record.resource_kind != intent.resource_kind
        {
            return Err(StateError::ResourceInvariant {
                detail: format!(
                    "resource key {:?} already has a different owner contract",
                    intent.key
                ),
            });
        }
        Ok(record)
    }

    /// Persist submission state while the create result is still unknown.
    pub fn resource_intent_payload(
        &self,
        owner: &str,
        key: &str,
        payload: &str,
    ) -> Result<(), StateError> {
        self.remember_payload_secrets(owner, payload)?;
        self.resource_transition(
            "UPDATE resources SET payload = ?3, updated_at = ?4 WHERE owner_id = ?1 AND resource_key = ?2 AND phase = 'intent'",
            &[owner.into(), key.into(), payload.into(), Self::now().into()], key,
        )
    }

    /// Save the returned handle before configuring, deploying, or waiting.
    pub fn resource_created(
        &self,
        owner: &str,
        key: &str,
        id: &str,
        payload: &str,
    ) -> Result<(), StateError> {
        self.remember_payload_secrets(owner, payload)?;
        self.resource_transition(
            "UPDATE resources SET phase = 'created', resource_id = ?3, payload = ?4, updated_at = ?5
             WHERE owner_id = ?1 AND resource_key = ?2 AND (phase = 'intent' OR (phase = 'created' AND resource_id = ?3))",
            &[owner.into(), key.into(), id.into(), payload.into(), Self::now().into()], key,
        )
    }

    /// Refresh credentials or recovery handles without changing the resource's phase.
    pub fn resource_refresh_payload(
        &self,
        owner: &str,
        key: &str,
        id: &str,
        payload: &str,
    ) -> Result<(), StateError> {
        self.remember_payload_secrets(owner, payload)?;
        self.resource_transition(
            "UPDATE resources SET payload = ?4, updated_at = ?5 WHERE owner_id = ?1 AND resource_key = ?2 AND resource_id = ?3 AND phase IN ('created', 'ready')",
            &[owner.into(), key.into(), id.into(), payload.into(), Self::now().into()], key,
        )
    }

    /// Recreate only after the previous resource was independently observed absent.
    pub fn resource_rearm(
        &self,
        owner: &str,
        key: &str,
        id: &str,
        payload: &str,
    ) -> Result<(), StateError> {
        self.resource_transition(
            "UPDATE resources SET phase = 'intent', resource_id = ?3, payload = ?4, updated_at = ?5
             WHERE owner_id = ?1 AND resource_key = ?2 AND phase = 'absent'",
            &[
                owner.into(),
                key.into(),
                id.into(),
                payload.into(),
                Self::now().into(),
            ],
            key,
        )
    }

    pub fn resource_ready(&self, owner: &str, key: &str) -> Result<(), StateError> {
        self.resource_transition(
            "UPDATE resources SET phase = 'ready', updated_at = ?3 WHERE owner_id = ?1 AND resource_key = ?2
             AND phase IN ('created', 'ready')",
            &[owner.into(), key.into(), Self::now().into()], key,
        )
    }

    /// Only call after verified absence or release of a borrowed/shared reference.
    pub fn resource_absent(&self, owner: &str, key: &str) -> Result<(), StateError> {
        self.resource_transition(
            "UPDATE resources SET phase = 'absent', updated_at = ?3 WHERE owner_id = ?1 AND resource_key = ?2",
            &[owner.into(), key.into(), Self::now().into()], key,
        )
    }

    fn resource_transition(
        &self,
        sql: &str,
        params: &[super::value::Value],
        key: &str,
    ) -> Result<(), StateError> {
        if self.execute(sql, params)? != 1 {
            return Err(StateError::ResourceInvariant {
                detail: format!("invalid transition for resource {key:?}"),
            });
        }
        Ok(())
    }

    pub fn resources(&self, owner: &str) -> Result<Vec<ResourceRecord>, StateError> {
        self.query_map(
            "SELECT owner_id, resource_key, step_id, provider, ownership, phase,
            resource_kind, resource_id, payload, created_at, updated_at, dependencies
            FROM resources WHERE owner_id = ?1 ORDER BY rowid",
            &[owner.into()],
            ResourceRecord::from_row,
        )
    }

    pub fn resource(&self, owner: &str, key: &str) -> Result<Option<ResourceRecord>, StateError> {
        self.query_row(
            "SELECT owner_id, resource_key, step_id, provider, ownership, phase,
            resource_kind, resource_id, payload, created_at, updated_at, dependencies
            FROM resources WHERE owner_id = ?1 AND resource_key = ?2",
            &[owner.into(), key.into()],
            ResourceRecord::from_row,
        )
    }
}
