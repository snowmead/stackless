//! The interpolation namespace (ARCHITECTURE.md §1).
//!
//! Env values reference a namespace evaluated per instance per
//! substrate. `$PORT` is deliberately *not* an interpolation reference —
//! it is injected by the local substrate into `run` commands only, so
//! the tokenizer here only understands `${...}` forms.

use std::collections::BTreeMap;

use super::error::DefError;
use crate::types::DnsName;

/// A parsed `${...}` reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reference {
    /// `${stack.name}`
    StackName,
    /// `${instance.name}`
    InstanceName,
    /// `${services.X.origin}`
    ServiceOrigin(String),
    /// `${datastores.X.url}`
    DatastoreUrl(String),
    /// `${secrets.KEY}`
    Secret(String),
    /// `${integrations.X.output}`
    IntegrationOutput { integration: String, output: String },
}

/// Extract every `${...}` reference from a value.
///
/// `location` names where the value lives (e.g. `services.api.env.DATABASE_URL`)
/// so errors point at the right line of the definition.
pub fn references(value: &str, location: &str) -> Result<Vec<Reference>, DefError> {
    let mut refs = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(DefError::ReferenceSyntax {
                location: location.to_owned(),
                reference: rest[start..].to_owned(),
                detail: "unterminated ${...}".into(),
            });
        };
        refs.push(parse_reference(&after[..end], location)?);
        rest = &after[end + 1..];
    }
    Ok(refs)
}

fn parse_reference(inner: &str, location: &str) -> Result<Reference, DefError> {
    let parts: Vec<&str> = inner.split('.').collect();
    let reference = match parts.as_slice() {
        ["instance", "name"] => Reference::InstanceName,
        ["stack", "name"] => Reference::StackName,
        ["services", name, "origin"] => Reference::ServiceOrigin((*name).to_owned()),
        ["datastores", name, "url"] => Reference::DatastoreUrl((*name).to_owned()),
        ["secrets", key] => Reference::Secret((*key).to_owned()),
        ["integrations", name, output] => Reference::IntegrationOutput {
            integration: (*name).to_owned(),
            output: (*output).to_owned(),
        },
        _ => {
            return Err(DefError::ReferenceSyntax {
                location: location.to_owned(),
                reference: format!("${{{inner}}}"),
                detail: "not a recognized namespace form".into(),
            });
        }
    };
    Ok(reference)
}

/// The values a substrate supplies for one instance. Built per instance
/// per substrate; resolution itself is substrate-blind.
#[derive(Debug)]
pub struct Namespace {
    pub stack_name: DnsName,
    pub instance_name: DnsName,
    pub service_origins: BTreeMap<String, String>,
    pub datastore_urls: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, String>,
    pub integrations: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for Namespace {
    fn default() -> Self {
        Self {
            stack_name: DnsName::try_new("stack").expect("placeholder stack name"),
            instance_name: DnsName::try_new("instance").expect("placeholder instance name"),
            service_origins: BTreeMap::new(),
            datastore_urls: BTreeMap::new(),
            secrets: BTreeMap::new(),
            integrations: BTreeMap::new(),
        }
    }
}

impl Namespace {
    fn lookup(&self, reference: &Reference, location: &str) -> Result<String, DefError> {
        match reference {
            Reference::StackName => Ok(self.stack_name.as_str().to_owned()),
            Reference::InstanceName => Ok(self.instance_name.as_str().to_owned()),
            Reference::ServiceOrigin(name) => {
                self.service_origins.get(name).cloned().ok_or_else(|| {
                    DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "service",
                        name: name.clone(),
                    }
                })
            }
            Reference::DatastoreUrl(name) => {
                self.datastore_urls.get(name).cloned().ok_or_else(|| {
                    DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "datastore",
                        name: name.clone(),
                    }
                })
            }
            Reference::Secret(key) => {
                self.secrets
                    .get(key)
                    .cloned()
                    .ok_or_else(|| DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "secret",
                        name: key.clone(),
                    })
            }
            Reference::IntegrationOutput {
                integration,
                output,
            } => self
                .integrations
                .get(integration)
                .and_then(|outputs| outputs.get(output))
                .cloned()
                .ok_or_else(|| DefError::UndeclaredReference {
                    location: location.to_owned(),
                    kind: "integration output",
                    name: format!("{integration}.{output}"),
                }),
        }
    }

    /// Load integration output checkpoints into the interpolation namespace.
    ///
    /// Payload shape is provider-agnostic:
    /// `{ "outputs": { "secret_key": "...", "publishable_key": "..." } }`.
    pub fn add_integration_checkpoints(&mut self, checkpoints: &[crate::state::Checkpoint]) {
        for checkpoint in checkpoints {
            let Some(name) = checkpoint.step_id.strip_prefix("integration:") else {
                continue;
            };
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(&checkpoint.payload) else {
                continue;
            };
            let Some(outputs) = payload.get("outputs").and_then(|value| value.as_object()) else {
                continue;
            };
            let entry = self.integrations.entry(name.to_owned()).or_default();
            for (key, value) in outputs {
                if let Some(value) = value.as_str() {
                    entry.insert(key.clone(), value.to_owned());
                }
            }
        }
    }
}

/// Substitute every `${...}` in `value` from the namespace.
pub fn resolve(value: &str, namespace: &Namespace, location: &str) -> Result<String, DefError> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(DefError::ReferenceSyntax {
                location: location.to_owned(),
                reference: rest[start..].to_owned(),
                detail: "unterminated ${...}".into(),
            });
        };
        let reference = parse_reference(&after[..end], location)?;
        out.push_str(&namespace.lookup(&reference, location)?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_all_namespace_forms() {
        let refs = references(
            "${stack.name} ${instance.name} ${services.web.origin} ${datastores.db.url} ${secrets.KEY} ${integrations.clerk.secret_key}",
            "test",
        )
        .unwrap();
        assert_eq!(
            refs,
            vec![
                Reference::StackName,
                Reference::InstanceName,
                Reference::ServiceOrigin("web".into()),
                Reference::DatastoreUrl("db".into()),
                Reference::Secret("KEY".into()),
                Reference::IntegrationOutput {
                    integration: "clerk".into(),
                    output: "secret_key".into(),
                },
            ]
        );
    }

    #[test]
    fn port_is_not_a_reference() {
        assert!(references("vite --port $PORT", "test").unwrap().is_empty());
    }

    #[test]
    fn unterminated_reference_fails() {
        assert!(references("${services.web.origin", "test").is_err());
    }

    #[test]
    fn unknown_form_fails() {
        assert!(references("${services.web.port}", "test").is_err());
    }

    #[test]
    fn resolves_mixed_text() {
        let mut namespace = Namespace {
            stack_name: DnsName::try_new("atto").unwrap(),
            instance_name: DnsName::try_new("demo").unwrap(),
            ..Namespace::default()
        };
        namespace
            .service_origins
            .insert("web".into(), "http://web.demo.localhost:4444".into());
        let resolved = resolve(
            "origin=${services.web.origin};i=${instance.name}",
            &namespace,
            "t",
        )
        .unwrap();
        assert_eq!(resolved, "origin=http://web.demo.localhost:4444;i=demo");
    }
}
