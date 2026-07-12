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
use stackless_provider_sdk::{
    BlockedSetting, ConfigScope, Hostable, IntegrationError, IntegrationHosting, ProviderOps,
    host_bound_hosts, host_bound_supports,
};
pub use stackless_provider_sdk::{config_bool, config_optional_string, config_string};

use crate::providers;

type ValidateFn = fn(&str, &BTreeMap<String, toml::Value>) -> Result<(), IntegrationError>;

/// One row in the provider table, materialized from a [`Hostable`] impl plus
/// its lifecycle [`ProviderOps`]. The registry is the single source of truth
/// for both metadata and behaviour — dispatch never matches provider strings.
struct ProviderEntry {
    provider: &'static str,
    hosting: IntegrationHosting,
    config_scope: ConfigScope,
    resource_kind: &'static str,
    outputs: &'static [&'static str],
    blocked_settings: &'static [BlockedSetting],
    validate_config: ValidateFn,
    ops: &'static dyn ProviderOps,
}

const fn provider_entry<T: Hostable>(
    validate_config: ValidateFn,
    ops: &'static dyn ProviderOps,
) -> ProviderEntry {
    ProviderEntry {
        provider: T::PROVIDER,
        hosting: T::HOSTING,
        config_scope: T::CONFIG_SCOPE,
        resource_kind: T::RESOURCE_KIND,
        outputs: T::OUTPUTS,
        blocked_settings: T::BLOCKED_SETTINGS,
        validate_config,
        ops,
    }
}

macro_rules! register_providers {
    ( $( ( $($m:ident)::+ , $T:ident ) ),+ $(,)? ) => {
        const PROVIDERS: &[ProviderEntry] = &[
            $(
                provider_entry::<providers::$($m)::+::$T>(
                    providers::$($m)::+::validate_config,
                    &providers::$($m)::+::$T,
                ),
            )+
        ];
    };
}

register_providers! {
    (clerk, ClerkAuth),
    (cloudflare::browser_run, CloudflareBrowserRun),
    (cloudflare::d1, CloudflareD1),
    (cloudflare::hyperdrive, CloudflareHyperdrive),
    (cloudflare::kv, CloudflareKv),
    (cloudflare::queues, CloudflareQueues),
    (cloudflare::r2, CloudflareR2),
    (cloudflare::workers, CloudflareWorkers),
    (cloudflare::workers_ai, CloudflareWorkersAi),
    (agentmail::api, AgentMailApi),
    (agentphone::number, AgentPhoneNumber),
    (algolia::application, AlgoliaApplication),
    (amplitude::analytics, AmplitudeAnalytics),
    (auth0::client, Auth0Client),
    (base44_projects::app, Base44ProjectsApp),
    (blaxel::agent_drive, BlaxelAgentDrive),
    (blaxel::sandbox, BlaxelSandbox),
    (browserbase::project, BrowserbaseProject),
    (chroma::database, ChromaDatabase),
    (clickhouse::cluster, ClickHouseClickhouse),
    (clickhouse::postgres, ClickHousePostgres),
    (daytona::sandbox, DaytonaSandbox),
    (e2b::sandbox, E2BSandbox),
    (elevenlabs::tts, ElevenLabsTts),
    (exa::api, ExaApi),
    (firecrawl::api, FirecrawlApi),
    (flyio::mpg, FlyioMpg),
    (flyio::sprite, FlyioSprite),
    (gitlab::project, GitLabProject),
    (heygen::api, HeyGenApi),
    (huggingface::bucket, HuggingFaceBucket),
    (huggingface::platform, HuggingFacePlatform),
}

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
    active_host: Option<&str>,
    known_substrates: &[&str],
) -> Result<(), IntegrationError> {
    let entry = lookup(&integration.provider).ok_or_else(|| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}"),
        detail: format!("unsupported provider {:?}", integration.provider),
    })?;

    validate_host_blocks(
        name,
        integration,
        entry.hosting,
        entry.config_scope,
        known_substrates,
    )?;

    if let Some(host) = active_host
        && matches!(entry.hosting, IntegrationHosting::HostBound(_))
        && !host_bound_supports(entry.hosting, host)
    {
        return Err(IntegrationError::HostUnsupported {
            provider: integration.provider.clone(),
            host: host.to_owned(),
        });
    }

    let config = match (entry.config_scope, active_host) {
        (ConfigScope::PerHost, Some(host)) => integration.effective_config(host, known_substrates),
        _ => integration.config_fields(known_substrates),
    };
    (entry.validate_config)(name, &config)?;

    for setting in entry.blocked_settings {
        if config_bool(&config, setting.key) {
            return Err(IntegrationError::ConfigInvalid {
                location: format!("integrations.{name}.{}", setting.key),
                detail: setting.remediation.to_owned(),
            });
        }
    }
    Ok(())
}

pub fn validate_all(
    def: &StackDef,
    active_host: Option<&str>,
    known_substrates: &[&str],
) -> Result<(), IntegrationError> {
    for (name, integration) in &def.integrations {
        validate_integration(name, integration, active_host, known_substrates)?;
    }
    validate_integration_outputs(def, known_substrates)?;
    Ok(())
}

fn validate_host_blocks(
    name: &str,
    integration: &Integration,
    hosting: IntegrationHosting,
    scope: ConfigScope,
    known_substrates: &[&str],
) -> Result<(), IntegrationError> {
    for (host, _block) in integration.host_blocks(known_substrates) {
        if matches!(hosting, IntegrationHosting::Managed) {
            return Err(IntegrationError::ConfigInvalid {
                location: format!("integrations.{name}.{host}"),
                detail: format!(
                    "provider {:?} is managed and does not support per-host configuration",
                    integration.provider
                ),
            });
        }
        if matches!(scope, ConfigScope::GlobalOnly) {
            return Err(IntegrationError::ConfigInvalid {
                location: format!("integrations.{name}.{host}"),
                detail: format!(
                    "provider {:?} does not support per-host configuration",
                    integration.provider
                ),
            });
        }
        if !host_bound_supports(hosting, &host) {
            return Err(IntegrationError::ConfigInvalid {
                location: format!("integrations.{name}.{host}"),
                detail: format!(
                    "host {host:?} is not supported by provider {:?}",
                    integration.provider
                ),
            });
        }
        let _ = host_bound_hosts(hosting);
    }
    Ok(())
}

fn validate_integration_outputs(
    def: &StackDef,
    known_substrates: &[&str],
) -> Result<(), IntegrationError> {
    let mut locations = Vec::new();
    if let Some(verify) = &def.stack.verify {
        for (key, value) in &verify.env {
            locations.push((format!("stack.verify.env.{key}"), value.clone()));
        }
        for (tier, spec) in &verify.tiers {
            for (key, value) in &spec.env {
                locations.push((
                    format!("stack.verify.tiers.{tier}.env.{key}"),
                    value.clone(),
                ));
            }
        }
    }
    for (service_name, service) in &def.services {
        for (key, value) in &service.env {
            locations.push((format!("services.{service_name}.env.{key}"), value.clone()));
        }
        for &host in known_substrates {
            for (key, value) in service.substrate_env(service_name, host).map_err(|err| {
                IntegrationError::ConfigInvalid {
                    location: format!("services.{service_name}.{host}.env"),
                    detail: err.to_string(),
                }
            })? {
                locations.push((format!("services.{service_name}.{host}.env.{key}"), value));
            }
        }
    }
    for (name, integration) in &def.integrations {
        for (key, value) in integration.config_fields(known_substrates) {
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

/// The lifecycle ops for `provider`, for provisioning dispatch.
pub fn ops_for(provider: &str) -> Option<&'static dyn ProviderOps> {
    lookup(provider).map(|entry| entry.ops)
}

/// The lifecycle ops owning resources of `kind`, for observe/destroy dispatch.
pub fn ops_for_resource_kind(kind: &str) -> Option<&'static dyn ProviderOps> {
    PROVIDERS
        .iter()
        .find(|entry| entry.resource_kind == kind)
        .map(|entry| entry.ops)
}

/// The substrate keys that count as host overrides for `provider`'s config —
/// its declared host-bound hosts (empty for managed providers). The provision
/// path uses this since it has no global substrate list; the authoritative
/// misplaced-block check runs at `up`/`check` with the full substrate list.
pub fn provider_host_keys(provider: &str) -> &'static [&'static str] {
    lookup(provider)
        .map(|entry| host_bound_hosts(entry.hosting))
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use stackless_core::def::StackDef;
    use stackless_core::fault::{Fault, codes};

    use super::*;

    const KNOWN: &[&str] = &["local", "render", "vercel", "fly", "netlify"];

    /// Registry hygiene: every provider string and resource kind is unique, so a
    /// new `PROVIDERS` row can't silently shadow another's dispatch.
    #[test]
    fn registry_providers_and_resource_kinds_are_unique() {
        use std::collections::BTreeSet;
        let providers: BTreeSet<&str> = PROVIDERS.iter().map(|e| e.provider).collect();
        assert_eq!(
            providers.len(),
            PROVIDERS.len(),
            "duplicate provider string"
        );
        let kinds: BTreeSet<&str> = PROVIDERS.iter().map(|e| e.resource_kind).collect();
        assert_eq!(kinds.len(), PROVIDERS.len(), "duplicate resource_kind");
    }

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
        let err = validate_integration("clerk", &def.integrations["clerk"], Some("render"), KNOWN)
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
        let err = validate_integration("clerk", &integration, None, KNOWN).unwrap_err();
        assert_eq!(err.code(), codes::INTEGRATION_CONFIG_INVALID);
    }

    /// A provider's [`Hostable::BLOCKED_SETTINGS`] fail validation loudly (with
    /// the remediation) instead of being silently ignored. Exercises the generic
    /// `entry.blocked_settings` path, so it covers any future provider too.
    #[test]
    fn provider_rejects_blocked_setting() {
        let def = StackDef::parse(
            r#"
[stack]
name = "demo"
[integrations.clerk]
provider = "clerk"
app_name = "demo"
credential_set = "development"
username = true
[services.web]
source = { repo = "https://example.invalid/web", ref = "main" }
health = { path = "/" }
[services.web.local]
run = "true"
"#,
        )
        .unwrap();
        let err =
            validate_integration("clerk", &def.integrations["clerk"], None, KNOWN).unwrap_err();
        assert_eq!(err.code(), codes::INTEGRATION_CONFIG_INVALID);
        assert!(err.to_string().contains("Sign-in with username"));
    }
}
