//! Definition validation: everything that fails "at parse time, not at
//! `up` time" (ARCHITECTURE.md §1 resolution rules).
//!
//! Core knows no substrate by name (ground rule: the Substrate trait is
//! the only provider seam); callers pass the names of registered
//! substrates so unknown keys can be told apart from substrate blocks.

use std::collections::BTreeMap;

use super::error::DefError;
use super::interp::{self, Reference};
use super::model::{Service, StackDef};

/// Engines with built-in readiness in v0 (ARCHITECTURE.md §7).
const KNOWN_ENGINES: &[&str] = &["postgres"];

/// DNS-safe: it becomes hostnames and cloud service names.
pub fn dns_safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Validate the whole definition against the rules substrates share.
///
/// Per-substrate shape validation (does `[services.X.render]` carry what
/// Render needs?) belongs to the substrate implementations; core checks
/// here are substrate-blind.
pub fn validate(def: &StackDef, known_substrates: &[&str]) -> Result<(), DefError> {
    if !dns_safe(&def.stack.name) {
        return Err(DefError::NameInvalid {
            kind: "stack",
            name: def.stack.name.clone(),
        });
    }
    if def.services.is_empty() {
        return Err(DefError::NoServices);
    }

    validate_substrate_keys(&def.stack.substrates, "stack", known_substrates)?;

    for (name, datastore) in &def.datastores {
        if !dns_safe(name) {
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
        if !dns_safe(name) {
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

/// `up --on <s>` fails at validation if any service lacks the config
/// that substrate requires (ARCHITECTURE.md §2). Core checks presence;
/// the substrate's own validation checks shape.
pub fn validate_for_substrate(def: &StackDef, substrate: &str) -> Result<(), DefError> {
    for (name, service) in &def.services {
        if !service.substrates.contains_key(substrate) {
            return Err(DefError::SubstrateConfigMissing {
                service: name.clone(),
                substrate: substrate.to_owned(),
            });
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

fn validate_references(def: &StackDef, refs: &[Reference], location: &str) -> Result<(), DefError> {
    for reference in refs {
        match reference {
            Reference::InstanceName => {}
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
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::dns_safe;

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
