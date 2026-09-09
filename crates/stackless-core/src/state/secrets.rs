//! Private redaction history. Never serialize these values in controller replies.

use super::{StateError, Store};
use crate::security::Redactor;

impl Store {
    pub(crate) fn remember_payload_secrets(
        &self,
        owner_id: &str,
        payload: &str,
    ) -> Result<(), StateError> {
        if let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(payload) {
            for (key, value) in fields {
                if key == "outputs" {
                    if let Some(outputs) = value.as_object() {
                        self.remember_secrets(
                            owner_id,
                            outputs.values().filter_map(serde_json::Value::as_str),
                        )?;
                    }
                } else if (crate::security::sensitive_key(&key) || key.ends_with("url"))
                    && let Some(value) = value.as_str()
                {
                    self.remember_secrets(owner_id, [value])?;
                }
            }
        }
        Ok(())
    }

    /// Public URL references stay usable in operation results. Literal env values,
    /// secret references, and sensitive keys still enter redaction history.
    /// This never removes a value already recorded as a secret.
    pub fn remember_environment(
        &self,
        owner_id: &str,
        environment: &std::collections::BTreeMap<String, String>,
        raw: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), StateError> {
        self.remember_secrets(
            owner_id,
            environment.iter().filter_map(|(key, value)| {
                let public = !crate::security::sensitive_key(key)
                    && raw.get(key).is_some_and(|raw| {
                        let Ok(references) = crate::def::interp::references(raw, "environment")
                        else {
                            return false;
                        };
                        match references.as_slice() {
                            [crate::def::Reference::EndpointUrl(name)] => {
                                raw == &format!("${{endpoints.{name}.url}}")
                            }
                            [crate::def::Reference::ServiceOrigin(name)] => {
                                raw == &format!("${{services.{name}.origin}}")
                            }
                            _ => false,
                        }
                    });
                (!public).then_some(value.as_str())
            }),
        )
    }

    /// Must finish before a secret is injected into user code.
    pub fn remember_secrets<'a>(
        &self,
        owner_id: &str,
        values: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), StateError> {
        for value in values {
            if !value.is_empty() {
                self.execute(
                    "INSERT INTO secret_history(owner_id, value) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                    &[owner_id.into(), value.into()],
                )?;
            }
        }
        Ok(())
    }

    /// Include retired births: provider logs can contain values from old revisions.
    pub fn redactor(&self) -> Result<Redactor, StateError> {
        let values = self.query_map("SELECT DISTINCT value FROM secret_history", &[], |row| {
            row.get_string(0)
        })?;
        Ok(Redactor::new(values))
    }

    pub fn bind_operation_instance(&self, id: &str, owner_id: &str) -> Result<(), StateError> {
        if self.execute("UPDATE operations SET instance_id = ?2 WHERE id = ?1 AND status = 'running' AND (instance_id IS NULL OR instance_id = ?2)", &[id.into(), owner_id.into()])? != 1 {
            return Err(StateError::ResourceInvariant { detail: "operation cannot change its instance identity".into() });
        }
        Ok(())
    }
}
