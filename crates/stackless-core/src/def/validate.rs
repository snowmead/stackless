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

impl StackDef {
    /// Validate the whole definition against the rules registered substrates share.
    /// Callers pass the names of registered substrates (core knows none by name).
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
        for (tier, spec) in &verify.tiers {
            for (key, value) in &spec.env {
                let location = format!("stack.verify.tiers.{tier}.env.{key}");
                let refs = interp::references(value, &location)?;
                validate_references(def, &refs, &location)?;
            }
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
        validate_integration_string_refs(def, name, integration, known_substrates)?;
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
    known_substrates: &[&str],
) -> Result<(), DefError> {
    for (key, value) in integration.config_fields(known_substrates) {
        let Some(text) = value.as_str() else {
            continue;
        };
        let location = format!("integrations.{name}.{key}");
        let refs = interp::references(text, &location)?;
        validate_references(def, &refs, &location)?;
    }
    for substrate in known_substrates {
        let Some(block) = integration.host_block(substrate) else {
            continue;
        };
        for (key, value) in block {
            let Some(text) = value.as_str() else {
                continue;
            };
            let location = format!("integrations.{name}.{substrate}.{key}");
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
                // Fresh files cannot declare `[datastores.*]`; only
                // snapshot resume populates `legacy_datastores`.
                if !def.legacy_datastores.contains(target) {
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
