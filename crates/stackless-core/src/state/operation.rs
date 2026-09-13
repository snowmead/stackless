//! Accepted requests and reconnectable progress belong to the controller.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::row::Row;
use super::{StateError, Store};

const RETIRABLE: &str = "input_retired = 0
    AND status NOT IN ('queued', 'running') AND updated_at <= ?1
    AND NOT EXISTS (SELECT 1 FROM operations pending WHERE pending.instance = operations.instance AND pending.status IN ('queued', 'running'))
    AND NOT EXISTS (SELECT 1 FROM instances live WHERE live.instance_id = operations.instance_id OR (operations.instance_id IS NULL AND live.name = operations.instance))
    AND NOT EXISTS (SELECT 1 FROM resources r WHERE r.owner_id = operations.instance_id AND r.phase != 'absent')";

const JOURNALED_VERIFY: &str = "verb = 'verify' AND EXISTS (SELECT 1 FROM operation_events e
    WHERE e.operation_id = operations.id AND json_extract(e.event_json, '$.event') = 'VerificationJournal'
    AND json_extract(e.event_json, '$.version') = 1)";

fn request_digest(request: &serde_json::Value) -> String {
    format!("{:x}", Sha256::digest(request.to_string().as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl OperationStatus {
    pub fn terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub instance_id: Option<String>,
    pub output_version: i64,
    pub id: String,
    pub instance: String,
    pub verb: String,
    pub status: OperationStatus,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub cancel_requested: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationEvent {
    pub sequence: i64,
    pub event: serde_json::Value,
}

fn json(text: &str) -> Result<serde_json::Value, StateError> {
    serde_json::from_str(text).map_err(|err| StateError::ResourceInvariant {
        detail: format!("invalid operation JSON: {err}"),
    })
}

impl Operation {
    fn from_row(row: &Row) -> Result<Self, StateError> {
        let status = match row.get_string(3)?.as_str() {
            "queued" => OperationStatus::Queued,
            "running" => OperationStatus::Running,
            "succeeded" => OperationStatus::Succeeded,
            "failed" => OperationStatus::Failed,
            "cancelled" => OperationStatus::Cancelled,
            "interrupted" => OperationStatus::Interrupted,
            other => {
                return Err(StateError::ResourceInvariant {
                    detail: format!("invalid operation status {other:?}"),
                });
            }
        };
        let result = json(&row.get_string(4)?)?;
        let error = json(&row.get_string(5)?)?;
        Ok(Self {
            instance_id: row.get_opt_string(9)?,
            output_version: row.get_i64(10)?,
            id: row.get_string(0)?,
            instance: row.get_string(1)?,
            verb: row.get_string(2)?,
            status,
            result: (!result.is_null()).then_some(result),
            error: (!error.is_null()).then_some(error),
            cancel_requested: row.get_i64(6)? != 0,
            created_at: row.get_i64(7)?,
            updated_at: row.get_i64(8)?,
        })
    }
}

impl Store {
    /// A client-chosen ID makes retry after a lost submit response idempotent.
    pub fn submit_operation(
        &self,
        id: &str,
        instance: &str,
        verb: &str,
        request: &serde_json::Value,
    ) -> Result<Operation, StateError> {
        let request_json = request.to_string();
        self.execute("INSERT INTO operations(id, instance, verb, status, request_json, created_at, updated_at, request_digest)
            VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?5, ?6) ON CONFLICT(id) DO NOTHING",
            &[id.into(), instance.into(), verb.into(), request_json.as_str().into(), Self::now().into(), request_digest(request).into()])?;
        let operation = self
            .operation(id)?
            .ok_or_else(|| StateError::ResourceInvariant {
                detail: "accepted operation missing".into(),
            })?;
        if !self.operation_request_matches(id, request)?
            || operation.verb != verb
            || operation.instance != instance
        {
            return Err(StateError::ResourceInvariant {
                detail: format!("operation ID {id:?} was already used for a different request"),
            });
        }
        Ok(operation)
    }

    pub fn operation(&self, id: &str) -> Result<Option<Operation>, StateError> {
        self.query_row("SELECT id, instance, verb, status, COALESCE(result_json, 'null'),
            COALESCE(error_json, 'null'), cancel_requested, created_at, updated_at, instance_id, output_version FROM operations WHERE id = ?1",
            &[id.into()], Operation::from_row)
    }

    /// Internal execution input. Never included in public operation responses.
    pub fn operation_request(&self, id: &str) -> Result<serde_json::Value, StateError> {
        self.query_row(
            "SELECT request_json FROM operations WHERE id = ?1 AND input_retired = 0",
            &[id.into()],
            |row| json(&row.get_string(0)?),
        )?
        .ok_or_else(|| StateError::ResourceInvariant {
            detail: format!("operation {id:?} is unknown or its execution input has expired"),
        })
    }

    /// Compare a retry without requiring the retained source archive.
    pub fn operation_request_matches(
        &self,
        id: &str,
        request: &serde_json::Value,
    ) -> Result<bool, StateError> {
        Ok(self
            .query_row(
                "SELECT request_digest, request_json, input_retired FROM operations WHERE id = ?1",
                &[id.into()],
                |row| {
                    if let Some(digest) = row.get_opt_string(0)? {
                        Ok(digest == request_digest(request))
                    } else {
                        Ok(row.get_i64(2)? == 0 && json(&row.get_string(1)?)? == *request)
                    }
                },
            )?
            .unwrap_or(false))
    }

    /// Terminal inputs expire only after their birth has been collected.
    /// Unbound failures also wait until the alias has no instance or pending work.
    pub fn expired_operation_inputs(&self) -> Result<Vec<String>, StateError> {
        self.query_map(
            &format!(
                "SELECT id FROM operations WHERE {RETIRABLE} ORDER BY updated_at, id LIMIT 100"
            ),
            &[(Self::now() - super::TOMBSTONE_GC_WINDOW.as_secs() as i64).into()],
            |row| row.get_string(0),
        )
    }

    /// Call after deleting operation-owned files. A crash before this update
    /// leaves the original request available to retry that deletion.
    pub fn retire_operation_input(&self, id: &str) -> Result<bool, StateError> {
        let request = self.operation_request(id)?;
        Ok(self.execute(&format!("UPDATE operations SET request_digest = ?3, request_json = 'null', input_retired = 1 WHERE id = ?2 AND {RETIRABLE}"),
            &[(Self::now() - super::TOMBSTONE_GC_WINDOW.as_secs() as i64).into(), id.into(), request_digest(&request).into()])? == 1)
    }

    pub fn pending_operations(&self) -> Result<Vec<Operation>, StateError> {
        self.query_map(
            "SELECT id, instance, verb, status, COALESCE(result_json, 'null'),
            COALESCE(error_json, 'null'), cancel_requested, created_at, updated_at, instance_id, output_version FROM operations
            WHERE status IN ('queued', 'running') ORDER BY created_at, rowid",
            &[],
            Operation::from_row,
        )
    }

    pub fn operations(&self, instance: Option<&str>) -> Result<Vec<Operation>, StateError> {
        self.query_map(
            "SELECT id, instance, verb, status, COALESCE(result_json, 'null'),
            COALESCE(error_json, 'null'), cancel_requested, created_at, updated_at, instance_id, output_version FROM operations
            WHERE (?1 IS NULL OR instance = ?1) ORDER BY created_at DESC, rowid DESC LIMIT 100",
            &[instance.map_or(super::value::Value::Null, Into::into)],
            Operation::from_row,
        )
    }

    pub fn running_operation_id(&self, instance: &str) -> Result<Option<String>, StateError> {
        self.query_row(
            "SELECT id FROM operations WHERE instance = ?1 AND status = 'running'",
            &[instance.into()],
            |row| row.get_string(0),
        )
    }

    /// Only one worker can win this transition, even after duplicate submission.
    pub fn start_operation(&self, id: &str) -> Result<bool, StateError> {
        Ok(self.execute(&format!("UPDATE operations SET status = 'running', updated_at = ?2
            WHERE id = ?1 AND status = 'queued' AND (cancel_requested = 0 OR ({JOURNALED_VERIFY}))
            AND NOT EXISTS (SELECT 1 FROM operations active WHERE active.instance = operations.instance AND active.status = 'running')"), &[id.into(), Self::now().into()])? == 1)
    }

    /// Publish before verification can run a command. Older unjournaled proofs stay interrupted.
    pub fn mark_verification_journal(&self, id: &str) -> Result<(), StateError> {
        let operation = self
            .operation(id)?
            .ok_or_else(|| StateError::ResourceInvariant {
                detail: "verification operation disappeared".into(),
            })?;
        if operation.verb != "verify" || operation.status != OperationStatus::Running {
            return Err(StateError::ResourceInvariant {
                detail: "verification operation is not running".into(),
            });
        }
        self.execute(
            &format!(
                "INSERT INTO operation_events(operation_id, event_json, recorded_at)
            SELECT id, '{{\"event\":\"VerificationJournal\",\"version\":1}}', ?2 FROM operations
            WHERE id = ?1 AND NOT ({JOURNALED_VERIFY})"
            ),
            &[id.into(), Self::now().into()],
        )?;
        Ok(())
    }

    /// Called only while holding the controller's OS ownership lock after restart.
    pub fn recover_operations(&self) -> Result<(), StateError> {
        self.execute(&format!("UPDATE operations SET status = CASE WHEN ({JOURNALED_VERIFY}) THEN 'queued'
            WHEN cancel_requested = 1 THEN 'cancelled' WHEN verb IN ('up', 'down', 'gc') THEN 'queued' ELSE 'interrupted' END,
            updated_at = ?1 WHERE status = 'running'"), &[Self::now().into()])?;
        Ok(())
    }

    pub fn cancel_operation(&self, id: &str) -> Result<bool, StateError> {
        Ok(self.execute(
            &format!("UPDATE operations SET cancel_requested = 1,
            status = CASE WHEN status = 'queued' AND NOT ({JOURNALED_VERIFY}) THEN 'cancelled' ELSE status END, updated_at = ?2
            WHERE id = ?1 AND (status = 'queued' OR (status = 'running' AND (verb = 'up' OR ({JOURNALED_VERIFY}))))"),
            &[id.into(), Self::now().into()],
        )? == 1)
    }

    pub fn finish_operation(
        &self,
        id: &str,
        status: OperationStatus,
        result: Option<&serde_json::Value>,
        error: Option<&serde_json::Value>,
    ) -> Result<(), StateError> {
        if !status.terminal() {
            return Err(StateError::ResourceInvariant {
                detail: "finish requires terminal operation status".into(),
            });
        }
        let changed = self.execute(
            "UPDATE operations SET status = ?2, result_json = ?3, error_json = ?4, updated_at = ?5, output_version = 1
            WHERE id = ?1 AND status = 'running'",
            &[
                id.into(),
                status.as_str().into(),
                result.map_or(super::value::Value::Null, |v| v.to_string().into()),
                error.map_or(super::value::Value::Null, |v| v.to_string().into()),
                Self::now().into(),
            ],
        )?;
        if changed != 1 {
            return Err(StateError::ResourceInvariant {
                detail: format!("operation {id:?} is not running"),
            });
        }
        Ok(())
    }

    pub fn operation_event(&self, id: &str, event: &serde_json::Value) -> Result<(), StateError> {
        self.execute("INSERT INTO operation_events(operation_id, event_json, recorded_at) VALUES (?1, ?2, ?3)",
            &[id.into(), event.to_string().into(), Self::now().into()])?;
        Ok(())
    }

    pub fn operation_events(
        &self,
        id: &str,
        after: i64,
    ) -> Result<Vec<OperationEvent>, StateError> {
        self.query_map("SELECT sequence, event_json FROM operation_events WHERE operation_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT 256",
            &[id.into(), after.into()], |row| Ok(OperationEvent { sequence: row.get_i64(0)?, event: json(&row.get_string(1)?)? }))
    }
}

pub fn new_operation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
