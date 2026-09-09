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
            let target = service.on.as_deref().unwrap_or(substrate);
            if !service.substrates.contains_key(target)
                && service.run.is_none()
                && service.image.is_none()
            {
                return Err(DefError::SubstrateConfigMissing {
                    service: name.clone(),
                    substrate: target.to_owned(),
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
    if def.services.is_empty() && def.integrations.is_empty() {
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
        if service.kind == super::model::WorkloadKind::Service && service.health.is_none() {
            return Err(DefError::Schema {
                message: format!("services.{name}.health is required for a service"),
            });
        }
        if service.kind == super::model::WorkloadKind::Job && service.health.is_some() {
            return Err(DefError::Schema {
                message: format!(
                    "jobs.{name} completes with an exit status and cannot declare health"
                ),
            });
        }
        if let Some(health) = &service.health {
            health.validate(name)?;
        }
        if service.root_origin && service.health.as_ref().is_none_or(|health| health.is_tcp()) {
            return Err(DefError::Schema {
                message: format!("services.{name}.root_origin requires an HTTP listener"),
            });
        }
        if service.timeout_secs == 0 || service.timeout_secs > 86400 {
            return Err(DefError::Schema {
                message: format!("services.{name}.timeout_secs must be between 1 and 86400"),
            });
        }
        if service.image.as_ref().is_some_and(|image| {
            image.is_empty()
                || image
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
        }) {
            return Err(DefError::Schema {
                message: format!(
                    "services.{name}.image must be a nonempty image reference without whitespace"
                ),
            });
        }
        if !service.source.repo.is_empty() && service.source.path.is_some() {
            return Err(DefError::Schema {
                message: format!("services.{name}.source must select repo or path"),
            });
        }
        service.source_root(name, "")?;
        for provider in service.substrates.keys() {
            service.source_root(name, provider)?;
        }
        if service
            .on
            .as_ref()
            .is_some_and(|on| !known_substrates.contains(&on.as_str()))
        {
            return Err(DefError::Schema {
                message: format!("services.{name}.on names an unknown provider"),
            });
        }
        for (dependency, condition) in &service.depends_on {
            let Some(target) = def.services.get(dependency) else {
                return Err(DefError::UndeclaredReference {
                    location: format!("services.{name}.depends_on"),
                    kind: "workload",
                    name: dependency.clone(),
                });
            };
            if dependency == name
                || (*condition == super::model::DependencyCondition::Completed
                    && target.kind != super::model::WorkloadKind::Job)
                || (*condition != super::model::DependencyCondition::Completed
                    && target.kind == super::model::WorkloadKind::Job)
            {
                return Err(DefError::Schema {
                    message: format!(
                        "services.{name}.depends_on.{dependency} has an invalid condition for the target workload"
                    ),
                });
            }
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
    for (name, endpoint) in &def.endpoints {
        if !crate::types::dns_safe(name) || !def.services.contains_key(&endpoint.workload) {
            return Err(DefError::Schema {
                message: format!("endpoints.{name} must name an existing workload"),
            });
        }
        if def.services[&endpoint.workload].health.is_none() {
            return Err(DefError::Schema {
                message: format!("endpoints.{name} requires a workload with a health listener"),
            });
        }
        if let Some(raw) = &endpoint.url {
            let tcp = def.services[&endpoint.workload]
                .health
                .as_ref()
                .is_some_and(|health| health.is_tcp());
            let valid = url::Url::parse(raw).is_ok_and(|url| {
                (if tcp {
                    url.scheme() == "tcp"
                        && raw.starts_with("tcp://")
                        && url.port().is_some_and(|port| port != 0)
                        && url.path().is_empty()
                        && url.query().is_none()
                        && url.fragment().is_none()
                } else {
                    matches!(url.scheme(), "http" | "https")
                        && (raw.starts_with("http://") || raw.starts_with("https://"))
                }) && url.has_host()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && !raw.chars().any(char::is_whitespace)
            });
            if !valid {
                return Err(DefError::Schema {
                    message: format!(
                        "endpoints.{name}.url must match the workload protocol and contain a host without credentials; TCP URLs also require a nonzero port and no path, query, or fragment"
                    ),
                });
            }
        }
    }
    if root_origins.len() > 1 {
        return Err(DefError::RootOriginConflict {
            services: root_origins,
        });
    }

    if let Some(verify) = &def.stack.verify {
        if verify.timeout_secs == 0 || verify.timeout_secs > 86400 {
            return Err(DefError::Schema {
                message: "stack.verify.timeout_secs must be between 1 and 86400".into(),
            });
        }
        for (key, value) in &verify.env {
            let location = format!("stack.verify.env.{key}");
            let refs = interp::references(value, &location)?;
            validate_references(def, &refs, &location)?;
        }
        for (tier, spec) in &verify.tiers {
            if spec.timeout_secs == 0 || spec.timeout_secs > 86400 {
                return Err(DefError::Schema {
                    message: format!(
                        "stack.verify.tiers.{tier}.timeout_secs must be between 1 and 86400"
                    ),
                });
            }
            if !crate::types::dns_safe(tier) {
                return Err(DefError::NameInvalid {
                    kind: "verify tier",
                    name: tier.clone(),
                });
            }
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
                if def.services[target].health.is_none() {
                    return Err(DefError::Schema {
                        message: format!("{location}: workload {target:?} has no listener origin"),
                    });
                }
            }
            Reference::EndpointUrl(target) => {
                if location.starts_with("integrations.") {
                    return Err(DefError::Schema {
                        message: format!(
                            "{location}: endpoint URL references are supported only in workload and verification environments"
                        ),
                    });
                }
                if !def.endpoints.contains_key(target) {
                    return Err(DefError::UndeclaredReference {
                        location: location.to_owned(),
                        kind: "endpoint",
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
    use crate::def::StackDef;
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

    #[test]
    fn verification_deadlines_default_and_reject_invalid_default_or_tier_budgets() {
        let base = "[stack]\nname='verify-test'\n[jobs.task]\nrun='true'\n";
        let def = StackDef::parse(&format!(
            "{base}\n[stack.verify]\nrun='true'\n[stack.verify.tiers.integration]\nrun='true'\n"
        ))
        .unwrap();
        def.validate_hosts(&["local"]).unwrap();
        let verify = def.stack.verify.unwrap();
        assert_eq!(verify.resolve(None).unwrap().timeout_secs, 300);
        assert_eq!(
            verify.resolve(Some("integration")).unwrap().timeout_secs,
            300
        );
        for table in ["stack.verify", "stack.verify.tiers.integration"] {
            for budget in [0, 86401] {
                let def = StackDef::parse(&format!(
                    "{base}\n[{table}]\nrun='true'\ntimeout_secs={budget}\n"
                ))
                .unwrap();
                assert!(
                    def.validate_hosts(&["local"])
                        .unwrap_err()
                        .to_string()
                        .contains("timeout_secs")
                );
            }
        }
    }

    #[test]
    fn verify_tier_keys_must_be_dns_safe() {
        let text = r#"
[stack]
name = "bad"

[stack.verify.tiers."a);func"]
run = "true"

[services.web]
source = { repo = "https://example.invalid/x", ref = "main" }
health = { path = "/" }

[services.web.local]
run = "true"
"#;
        let def = StackDef::parse(text).expect("parse");
        let err = def.validate_hosts(&["local"]).expect_err("tier name");
        assert!(matches!(
            err,
            crate::def::DefError::NameInvalid {
                kind: "verify tier",
                ..
            }
        ));
    }
}
