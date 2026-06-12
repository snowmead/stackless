//! Definition validation: everything that fails "at parse time, not at
//! `up` time" (ARCHITECTURE.md §1 resolution rules).
//!
//! Core knows no substrate by name (ground rule: the Substrate trait is
//! the only provider seam); callers pass the names of registered
//! substrates so unknown keys can be told apart from substrate blocks.

use std::collections::BTreeMap;

use super::error::DefError;
use super::interp::{self, Reference};
use super::model::{Integration, Service, StackDef};
use crate::host::Host;

/// Engines with built-in readiness in v0 (ARCHITECTURE.md §7).
const KNOWN_ENGINES: &[&str] = &["postgres"];

impl StackDef {
    /// Validate the whole definition against the rules registered hosts share.
    pub fn validate(&self) -> Result<(), DefError> {
        let known_substrates: Vec<&str> = Host::ALL.iter().map(|host| host.as_str()).collect();
        validate_definition(self, &known_substrates)
    }

    /// Validate against an explicit host list (tests and tooling only).
    pub fn validate_hosts(&self, known_substrates: &[&str]) -> Result<(), DefError> {
        validate_definition(self, known_substrates)
    }

    /// `up --on <s>` fails at validation if any service lacks the config
    /// that substrate requires (ARCHITECTURE.md §2).
    pub fn validate_for_substrate(&self, substrate: &str) -> Result<(), DefError> {
        for (name, service) in &self.services {
            if !service.substrates.contains_key(substrate) {
                return Err(DefError::SubstrateConfigMissing {
                    service: name.clone(),
                    substrate: substrate.to_owned(),
                });
            }
        }
        Ok(())
    }
}

fn validate_definition(def: &StackDef, known_substrates: &[&str]) -> Result<(), DefError> {
    if !crate::types::dns_safe(def.stack.name.as_str()) {
        return Err(DefError::NameInvalid {
            kind: "stack",
            name: def.stack.name.as_str().to_owned(),
        });
    }
    if def.services.is_empty() {
        return Err(DefError::NoServices);
    }

    validate_substrate_keys(&def.stack.substrates, "stack", known_substrates)?;
    validate_integrations(def, known_substrates)?;

    for (name, datastore) in &def.datastores {
        if !crate::types::dns_safe(name) {
            return Err(DefError::NameInvalid {
                kind: "datastore",
                name: name.clone(),
            });
        }
        if !KNOWN_ENGINES.contains(&datastore.engine.as_str()) {
            return Err(DefError::EngineUnknown {
                datastore: name.clone(),
                engine: datastore.engine.clone(),
            });
        }
        validate_substrate_keys(
            &datastore.substrates,
            &format!("datastores.{name}"),
            known_substrates,
        )?;
    }

    let mut root_origins = Vec::new();
    for (name, service) in &def.services {
        if !crate::types::dns_safe(name) {
            return Err(DefError::NameInvalid {
                kind: "service",
                name: name.clone(),
            });
        }
        if service.root_origin {
            root_origins.push(name.clone());
        }
        validate_substrate_keys(
            &service.substrates,
            &format!("services.{name}"),
            known_substrates,
        )?;
        validate_service_references(def, name, service, known_substrates)?;
    }
    if root_origins.len() > 1 {
        return Err(DefError::RootOriginConflict {
            services: root_origins,
        });
    }

    if let Some(verify) = &def.stack.verify {
        for (key, value) in &verify.env {
            let location = format!("stack.verify.env.{key}");
            let refs = interp::references(value, &location)?;
            validate_references(def, &refs, &location)?;
        }
    }

    Ok(())
}

fn validate_substrate_keys(
    substrates: &BTreeMap<String, toml::Value>,
    location: &str,
    known_substrates: &[&str],
) -> Result<(), DefError> {
    for (key, value) in substrates {
        if key == "depends_on" {
            // A dependency must be expressed in wiring; an ordering need
            // with no wiring expression is a definition bug (§1).
            return Err(DefError::DependsOnRejected {
                location: location.to_owned(),
            });
        }
        if !known_substrates.contains(&key.as_str()) {
            return Err(DefError::UnknownKey {
                location: location.to_owned(),
                key: key.clone(),
                known_substrates: known_substrates.iter().map(|s| (*s).to_owned()).collect(),
            });
        }
        if !value.is_table() {
            return Err(DefError::SubstrateBlockInvalid {
                location: format!("{location}.{key}"),
                found: value.type_str().to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_service_references(
    def: &StackDef,
    name: &str,
    service: &Service,
    known_substrates: &[&str],
) -> Result<(), DefError> {
    // Injected same-named secrets must be resolvable before anything
    // provisions, so they must be in the required list.
    for key in &service.secrets {
        if !def.secrets.required.contains(key) {
            return Err(DefError::SecretNotRequired {
                location: format!("services.{name}.secrets"),
                key: key.clone(),
            });
        }
    }
    for (key, value) in &service.env {
        let location = format!("services.{name}.env.{key}");
        let refs = interp::references(value, &location)?;
        validate_references(def, &refs, &location)?;
    }
    // Substrate env overlays participate in wiring (§1: substrate env
    // blocks overlay the common env), so their references validate too.
    for substrate in known_substrates {
        let overlay = service.substrate_env(name, substrate)?;
        for (key, value) in &overlay {
            let location = format!("services.{name}.{substrate}.env.{key}");
            let refs = interp::references(value, &location)?;
            validate_references(def, &refs, &location)?;
        }
    }
    Ok(())
}

fn validate_integrations(def: &StackDef, known_substrates: &[&str]) -> Result<(), DefError> {
    for (name, integration) in &def.integrations {
        if !crate::types::dns_safe(name) {
            return Err(DefError::NameInvalid {
                kind: "integration",
                name: name.clone(),
            });
        }
        if integration.provider.is_empty() {
            return Err(DefError::IntegrationInvalid {
                integration: name.clone(),
                detail: "provider is required".into(),
            });
        }
        validate_integration_substrate_keys(name, integration, known_substrates)?;
        validate_integration_string_refs(def, name, integration)?;
    }
    Ok(())
}

fn validate_integration_substrate_keys(
    name: &str,
    integration: &Integration,
    known_substrates: &[&str],
) -> Result<(), DefError> {
    let substrates: std::collections::BTreeMap<String, toml::Value> = integration
        .fields
        .iter()
        .filter(|(key, _)| known_substrates.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    validate_substrate_keys(
        &substrates,
        &format!("integrations.{name}"),
        known_substrates,
    )
}

fn validate_integration_string_refs(
    def: &StackDef,
    name: &str,
    integration: &Integration,
) -> Result<(), DefError> {
    for (key, value) in integration.config_fields() {
        let Some(text) = value.as_str() else {
            continue;
        };
        let location = format!("integrations.{name}.{key}");
        let refs = interp::references(text, &location)?;
        validate_references(def, &refs, &location)?;
    }
    for host in Host::ALL {
        let Some(block) = integration.host_block(*host) else {
            continue;
        };
        for (key, value) in block {
            let Some(text) = value.as_str() else {
                continue;
            };
            let location = format!("integrations.{name}.{}.{}", host.as_str(), key);
            let refs = interp::references(text, &location)?;
            validate_references(def, &refs, &location)?;
        }
    }
    Ok(())
}

fn validate_references(def: &StackDef, refs: &[Reference], location: &str) -> Result<(), DefError> {
    for reference in refs {
        match reference {
            Reference::StackName | Reference::InstanceName => {}
            Reference::ServiceOrigin(target) => {
                if !def.services.contains_key(target) {
                    return Err(DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "service",
                        name: target.clone(),
                    });
                }
            }
            Reference::DatastoreUrl(target) => {
                if !def.datastores.contains_key(target) {
                    return Err(DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "datastore",
                        name: target.clone(),
                    });
                }
            }
            Reference::Secret(key) => {
                if !def.secrets.required.contains(key) {
                    return Err(DefError::SecretNotRequired {
                        location: location.to_owned(),
                        key: key.clone(),
                    });
                }
            }
            Reference::IntegrationOutput { integration, .. } => {
                if !def.integrations.contains_key(integration) {
                    return Err(DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "integration",
                        name: integration.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::types::dns_safe;

    #[test]
    fn dns_safety() {
        assert!(dns_safe("atto"));
        assert!(dns_safe("a1-b2"));
        assert!(!dns_safe(""));
        assert!(!dns_safe("Atto"));
        assert!(!dns_safe("1atto"));
        assert!(!dns_safe("atto-"));
        assert!(!dns_safe("at to"));
        assert!(!dns_safe(&"a".repeat(64)));
    }
}
