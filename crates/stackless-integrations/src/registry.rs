//! First-class integration provider registry: validation, outputs, dispatch.
//!
//! Validation rules:
//! 1. `provider` must match a registered [`Hostable`] entry.
//! 2. Host-key tables in `integration.fields` are rejected for [`IntegrationHosting::Managed`]
//!    and for [`ConfigScope::GlobalOnly`]; for [`ConfigScope::PerHost`] only hosts in
//!    [`IntegrationHosting::HostBound`] are allowed.
//! 3. `check` / `up --on <host>` requires membership in the host list for host-bound providers;
//!    managed providers skip this check.
//! 4. Provider-specific config validation runs on the effective config for the active host.

use std::collections::BTreeMap;

use stackless_core::def::interp::{self, Reference};
use stackless_core::def::{Integration, StackDef};
use stackless_core::host::Host;

use crate::error::IntegrationError;
use crate::hostable::{
    ConfigScope, Hostable, IntegrationHosting, host_bound_hosts, host_bound_supports,
};
use crate::providers;

type ValidateFn = fn(&str, &BTreeMap<String, toml::Value>) -> Result<(), IntegrationError>;

/// One row in the provider table, materialized from a [`Hostable`] impl.
struct ProviderEntry {
    provider: &'static str,
    hosting: IntegrationHosting,
    config_scope: ConfigScope,
    resource_kind: &'static str,
    outputs: &'static [&'static str],
    validate_config: ValidateFn,
}

const fn provider_entry<T: Hostable>(validate_config: ValidateFn) -> ProviderEntry {
    ProviderEntry {
        provider: T::PROVIDER,
        hosting: T::HOSTING,
        config_scope: T::CONFIG_SCOPE,
        resource_kind: T::RESOURCE_KIND,
        outputs: T::OUTPUTS,
        validate_config,
    }
}

const PROVIDERS: &[ProviderEntry] = &[provider_entry::<providers::clerk::ClerkAuth>(
    providers::clerk::validate_config,
)];

fn lookup(provider: &str) -> Option<&'static ProviderEntry> {
    PROVIDERS.iter().find(|entry| entry.provider == provider)
}

pub fn is_integration_resource(kind: &str) -> bool {
    PROVIDERS.iter().any(|entry| entry.resource_kind == kind)
}

pub fn known_outputs(provider: &str) -> Option<&'static [&'static str]> {
    lookup(provider).map(|entry| entry.outputs)
}

pub fn validate_integration(
    name: &str,
    integration: &Integration,
    active_host: Option<Host>,
) -> Result<(), IntegrationError> {
    let entry = lookup(&integration.provider).ok_or_else(|| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}"),
        detail: format!("unsupported provider {:?}", integration.provider),
    })?;

    validate_host_blocks(name, integration, entry.hosting, entry.config_scope)?;

    if let Some(host) = active_host
        && matches!(entry.hosting, IntegrationHosting::HostBound(_))
        && !host_bound_supports(entry.hosting, host)
    {
        return Err(IntegrationError::HostUnsupported {
            provider: integration.provider.clone(),
            host,
        });
    }

    let config = match (entry.config_scope, active_host) {
        (ConfigScope::PerHost, Some(host)) => integration.effective_config(host),
        _ => integration.config_fields(),
    };
    (entry.validate_config)(name, &config)
}

pub fn validate_all(def: &StackDef, active_host: Option<Host>) -> Result<(), IntegrationError> {
    for (name, integration) in &def.integrations {
        validate_integration(name, integration, active_host)?;
    }
    validate_integration_outputs(def)?;
    Ok(())
}

fn validate_host_blocks(
    name: &str,
    integration: &Integration,
    hosting: IntegrationHosting,
    scope: ConfigScope,
) -> Result<(), IntegrationError> {
    for (host, _block) in integration.host_blocks() {
        if matches!(hosting, IntegrationHosting::Managed) {
            return Err(IntegrationError::ConfigInvalid {
                location: format!("integrations.{name}.{}", host.as_str()),
                detail: format!(
                    "provider {:?} is managed and does not support per-host configuration",
                    integration.provider
                ),
            });
        }
        if matches!(scope, ConfigScope::GlobalOnly) {
            return Err(IntegrationError::ConfigInvalid {
                location: format!("integrations.{name}.{}", host.as_str()),
                detail: format!(
                    "provider {:?} does not support per-host configuration",
                    integration.provider
                ),
            });
        }
        if !host_bound_supports(hosting, host) {
            return Err(IntegrationError::ConfigInvalid {
                location: format!("integrations.{name}.{}", host.as_str()),
                detail: format!(
                    "host {:?} is not supported by provider {:?}",
                    host.as_str(),
                    integration.provider
                ),
            });
        }
        let _ = host_bound_hosts(hosting);
    }
    Ok(())
}

fn validate_integration_outputs(def: &StackDef) -> Result<(), IntegrationError> {
    let mut locations = Vec::new();
    if let Some(verify) = &def.stack.verify {
        for (key, value) in &verify.env {
            locations.push((format!("stack.verify.env.{key}"), value.clone()));
        }
    }
    for (service_name, service) in &def.services {
        for (key, value) in &service.env {
            locations.push((format!("services.{service_name}.env.{key}"), value.clone()));
        }
        for host in Host::ALL {
            for (key, value) in
                service
                    .substrate_env(service_name, host.as_str())
                    .map_err(|err| IntegrationError::ConfigInvalid {
                        location: format!("services.{service_name}.{}.env", host.as_str()),
                        detail: err.to_string(),
                    })?
            {
                locations.push((
                    format!("services.{service_name}.{}.env.{key}", host.as_str()),
                    value,
                ));
            }
        }
    }
    for (name, integration) in &def.integrations {
        for (key, value) in integration.config_fields() {
            if let Some(text) = value.as_str() {
                locations.push((format!("integrations.{name}.{key}"), text.to_owned()));
            }
        }
    }

    for (location, value) in locations {
        let refs = interp::references(&value, &location).map_err(|err| {
            IntegrationError::ConfigInvalid {
                location: location.clone(),
                detail: err.to_string(),
            }
        })?;
        for reference in refs {
            let Reference::IntegrationOutput {
                integration,
                output,
            } = reference
            else {
                continue;
            };
            let Some(spec) = def.integrations.get(&integration) else {
                continue;
            };
            let outputs =
                known_outputs(&spec.provider).ok_or_else(|| IntegrationError::ConfigInvalid {
                    location: location.clone(),
                    detail: format!("integration {integration:?} has unsupported provider"),
                })?;
            if !outputs.contains(&output.as_str()) {
                return Err(IntegrationError::ConfigInvalid {
                    location,
                    detail: format!(
                        "unknown output {output:?} for integration {integration:?} \
                         (known: {outputs:?})"
                    ),
                });
            }
        }
    }
    Ok(())
}

pub fn dispatch_resource_kind(provider: &str) -> Option<&'static str> {
    lookup(provider).map(|entry| entry.resource_kind)
}

pub fn config_string(
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<String, IntegrationError> {
    config
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| IntegrationError::ConfigInvalid {
            location: format!("integrations.*.{key}"),
            detail: format!("{key} is required"),
        })
}

pub fn config_bool(config: &BTreeMap<String, toml::Value>, key: &str) -> bool {
    config
        .get(key)
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

pub fn config_optional_string(config: &BTreeMap<String, toml::Value>, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use stackless_core::def::StackDef;
    use stackless_core::fault::{Fault, codes};
    use stackless_core::host::Host;

    use super::*;

    #[test]
    fn managed_provider_rejects_host_block() {
        let def = StackDef::parse(
            r#"
[stack]
name = "demo"
[integrations.clerk]
provider = "clerk"
app_name = "demo"
[integrations.clerk.render]
credential_set = "development"
[services.web]
source = { repo = "https://example.invalid/web", ref = "main" }
health = { path = "/" }
[services.web.local]
run = "true"
"#,
        )
        .unwrap();
        let err = validate_integration("clerk", &def.integrations["clerk"], Some(Host::Render))
            .unwrap_err();
        assert_eq!(err.code(), codes::INTEGRATION_CONFIG_INVALID);
        assert!(err.to_string().contains("managed"));
    }

    #[test]
    fn global_only_managed_provider_rejects_local_host_block() {
        let integration = Integration {
            provider: "clerk".to_owned(),
            fields: BTreeMap::from([
                (
                    "app_name".to_owned(),
                    toml::Value::String("demo".to_owned()),
                ),
                (
                    "local".to_owned(),
                    toml::Value::Table(
                        [(
                            "app_name".to_owned(),
                            toml::Value::String("override".to_owned()),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                ),
            ]),
        };
        let err = validate_integration("clerk", &integration, None).unwrap_err();
        assert_eq!(err.code(), codes::INTEGRATION_CONFIG_INVALID);
    }
}
