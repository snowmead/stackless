//! The definition model: serde structs sized exactly to the schema in
//! ARCHITECTURE.md §1.
//!
//! A service is substrate-independent identity + wiring + health; how a
//! substrate runs it is nested per substrate and captured here as opaque
//! TOML (`substrates` maps). Core never interprets a substrate block
//! beyond two contracts that §1 fixes across all substrates: the block
//! must be a table, and an `env` key inside it overlays the common env.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::error::DefError;
use crate::types::{DnsName, HttpStatus};

/// Top level of `stackless.toml`. Unknown top-level sections are
/// rejected (an old binary cannot honor a section it does not know).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StackDef {
    pub stack: Stack,
    #[serde(default)]
    pub secrets: SecretsSpec,
    #[serde(default, alias = "resources")]
    pub integrations: BTreeMap<String, Integration>,
    #[serde(default, alias = "workloads")]
    pub services: BTreeMap<String, Service>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub jobs: BTreeMap<String, Service>,
    #[serde(default)]
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Names from a stripped legacy `[datastores.*]` section in an
    /// instance snapshot. Empty for fresh files. Lets
    /// `${datastores.*.url}` validate and resolve from journaled
    /// provision checkpoints on resume.
    #[serde(skip)]
    pub legacy_datastores: BTreeSet<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Stack {
    pub name: DnsName,
    #[serde(default)]
    pub projects: ProjectsSpec,
    pub verify: Option<VerifyRoot>,
    /// Per-substrate stack config (e.g. `[stack.render]` region),
    /// plus any unknown keys — validation tells them apart.
    #[serde(flatten)]
    pub substrates: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectsSpec {
    pub stripe: Option<StripeProjectSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StripeProjectSpec {
    pub project: Option<String>,
}

/// The proof contract, run by `stackless verify` (ARCHITECTURE.md §7).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifySpec {
    pub run: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_job_timeout")]
    pub timeout_secs: u64,
}

/// `[stack.verify]` plus optional named tiers under `[stack.verify.tiers.<name>]`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyRoot {
    pub run: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_job_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub tiers: BTreeMap<String, VerifySpec>,
}

impl Default for VerifyRoot {
    fn default() -> Self {
        Self {
            run: None,
            env: BTreeMap::new(),
            timeout_secs: default_job_timeout(),
            tiers: BTreeMap::new(),
        }
    }
}

impl VerifyRoot {
    pub fn is_declared(&self) -> bool {
        self.run.is_some() || !self.tiers.is_empty()
    }

    pub fn resolve(&self, tier: Option<&str>) -> Option<VerifySpec> {
        match tier {
            None | Some("default") => self.run.as_ref().map(|run| VerifySpec {
                run: run.clone(),
                env: self.env.clone(),
                timeout_secs: self.timeout_secs,
            }),
            Some(name) => self.tiers.get(name).cloned(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretsSpec {
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Integration {
    /// Catalog adapter (e.g. `clerk` → `clerk/auth`).
    pub provider: String,
    /// Host placement for a resource. Defaults to the instance placement.
    pub on: Option<String>,
    /// Provider config and optional per-host override tables
    /// (`[integrations.<name>.<host>]`), allowed only for host-bound
    /// providers that declare per-host config in the integrations registry.
    #[serde(flatten)]
    pub fields: BTreeMap<String, toml::Value>,
}

impl Integration {
    /// Config keys excluding registered host override tables. `known_substrates`
    /// names the keys that count as host overrides (substrate names), so they are
    /// stripped from the provider's own config.
    pub fn config_fields(&self, known_substrates: &[&str]) -> BTreeMap<String, toml::Value> {
        self.fields
            .iter()
            .filter(|(key, _)| !known_substrates.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn host_block(&self, host: &str) -> Option<&toml::Table> {
        self.fields.get(host).and_then(toml::Value::as_table)
    }

    /// Parent config merged with a host override table when present.
    pub fn effective_config(
        &self,
        host: &str,
        known_substrates: &[&str],
    ) -> BTreeMap<String, toml::Value> {
        let mut out = self.config_fields(known_substrates);
        if let Some(override_table) = self.host_block(host) {
            for (key, value) in override_table {
                out.insert(key.clone(), value.clone());
            }
        }
        out
    }

    /// Every host-key table nested under this integration.
    pub fn host_blocks(&self, known_substrates: &[&str]) -> BTreeMap<String, &toml::Table> {
        self.fields
            .iter()
            .filter_map(|(key, value)| {
                if !known_substrates.contains(&key.as_str()) {
                    return None;
                }
                Some((key.clone(), value.as_table()?))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Service {
    /// Container image. Without one, local execution requires a caller host grant.
    pub image: Option<String>,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub kind: WorkloadKind,
    pub run: Option<String>,
    pub on: Option<String>,
    #[serde(default)]
    pub depends_on: BTreeMap<String, DependencyCondition>,
    #[serde(default = "default_job_timeout")]
    pub timeout_secs: u64,
    /// Runs once after the service's source is materialized.
    pub setup: Option<String>,
    /// Runs on every `up`, after dependencies are ready, before start.
    pub prepare: Option<String>,
    /// Secrets injected as same-named env vars; must be in `[secrets].required`.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Services require a probe; workers may omit it and jobs cannot use it.
    pub health: Option<Health>,
    /// At most one service per stack also claims `http://{instance}.localhost`.
    #[serde(default)]
    pub root_origin: bool,
    /// Per-substrate run config (`[services.X.local]`, `[services.X.render]`, ...).
    #[serde(flatten)]
    pub substrates: BTreeMap<String, toml::Value>,
}

/// Code sources are git references (ARCHITECTURE.md §1).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default)]
    pub repo: String,
    #[serde(rename = "ref", default = "default_source_ref")]
    pub reference: String,
    pub path: Option<String>,
    pub root: Option<String>,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            repo: String::new(),
            reference: default_source_ref(),
            path: None,
            root: None,
        }
    }
}

fn default_source_ref() -> String {
    "HEAD".into()
}
fn default_job_timeout() -> u64 {
    300
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    #[default]
    Service,
    Worker,
    Job,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyCondition {
    Started,
    Ready,
    Completed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub workload: String,
    /// A caller-managed URL. Omit it to bind the provider-assigned origin.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointSource {
    Provider,
    Declared,
}

/// A URL binding. A declared URL does not imply a provisioned route or a probe.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ResolvedEndpoint {
    pub workload: String,
    pub url: String,
    pub source: EndpointSource,
}

impl StackDef {
    pub fn resolve_endpoints(
        &self,
        origins: &BTreeMap<String, String>,
    ) -> BTreeMap<String, ResolvedEndpoint> {
        self.endpoints
            .iter()
            .filter_map(|(name, endpoint)| {
                let (url, source) = if let Some(url) = &endpoint.url {
                    (url, EndpointSource::Declared)
                } else {
                    (origins.get(&endpoint.workload)?, EndpointSource::Provider)
                };
                (!url.is_empty()).then(|| {
                    (
                        name.clone(),
                        ResolvedEndpoint {
                            workload: endpoint.workload.clone(),
                            url: url.clone(),
                            source,
                        },
                    )
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthProtocol {
    #[default]
    Http,
    Tcp,
}

impl HealthProtocol {
    fn is_http(&self) -> bool {
        *self == Self::Http
    }
}

/// HTTP response assertions or a TCP connection probe on the workload listener.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    #[serde(default, skip_serializing_if = "HealthProtocol::is_http")]
    pub protocol: HealthProtocol,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_health_status")]
    pub status: HttpStatus,
    pub contains: Option<String>,
}

impl Health {
    pub fn is_tcp(&self) -> bool {
        self.protocol == HealthProtocol::Tcp
    }

    pub fn validate(&self, workload: &str) -> Result<(), DefError> {
        let invalid = if self.is_tcp() {
            !self.path.is_empty() || self.status != HttpStatus::OK || self.contains.is_some()
        } else {
            !self.path.starts_with('/') || self.path.chars().any(char::is_control)
        };
        if invalid {
            return Err(DefError::Schema {
                message: format!(
                    "services.{workload}.health requires an HTTP path starting with /, or protocol = 'tcp' without HTTP assertions"
                ),
            });
        }
        Ok(())
    }
}

fn default_health_status() -> HttpStatus {
    HttpStatus::OK
}

impl Service {
    /// Common and provider roots name the same directory. Neither silently overrides the other.
    pub fn source_root(&self, service: &str, provider: &str) -> Result<Option<String>, DefError> {
        let normalize = |raw: &str, location: &str| -> Result<String, DefError> {
            use std::path::Component;
            let invalid = || DefError::Schema {
                message: format!(
                    "{location} must name a relative directory inside the source tree"
                ),
            };
            if raw.trim().is_empty() || raw.contains('\\') || raw.chars().any(char::is_control) {
                return Err(invalid());
            }
            let mut parts = Vec::new();
            for part in std::path::Path::new(raw).components() {
                match part {
                    Component::CurDir => (),
                    Component::Normal(part) => {
                        let part = part.to_str().ok_or_else(invalid)?;
                        if crate::source_archive::excluded(part) {
                            return Err(invalid());
                        }
                        parts.push(part);
                    }
                    _ => return Err(invalid()),
                }
            }
            Ok(if parts.is_empty() {
                ".".into()
            } else {
                parts.join("/")
            })
        };
        let common = self
            .source
            .root
            .as_deref()
            .map(|root| normalize(root, &format!("services.{service}.source.root")))
            .transpose()?;
        let provider_root = self
            .substrates
            .get(provider)
            .and_then(|block| block.get("root"))
            .map(|root| {
                let location = format!("services.{service}.{provider}.root");
                let root = root.as_str().ok_or_else(|| DefError::Schema {
                    message: format!("{location} must be a string"),
                })?;
                normalize(root, &location)
            })
            .transpose()?;
        if common
            .as_ref()
            .zip(provider_root.as_ref())
            .is_some_and(|(a, b)| a != b)
        {
            return Err(DefError::Schema {
                message: format!(
                    "services.{service}.source.root conflicts with services.{service}.{provider}.root"
                ),
            });
        }
        Ok(common.or(provider_root))
    }

    /// The `env` overlay inside a substrate block, when present.
    ///
    /// §1 resolution rules: substrate `env` blocks overlay the common
    /// `env`. This is the one key core reads inside an otherwise opaque
    /// substrate block.
    pub fn substrate_env(
        &self,
        service_name: &str,
        substrate: &str,
    ) -> Result<BTreeMap<String, String>, DefError> {
        let Some(block) = self.substrates.get(substrate) else {
            return Ok(BTreeMap::new());
        };
        let location = format!("services.{service_name}.{substrate}.env");
        let Some(table) = block.as_table() else {
            // Non-table substrate blocks are rejected by validation;
            // treat as no overlay here.
            return Ok(BTreeMap::new());
        };
        let Some(env) = table.get("env") else {
            return Ok(BTreeMap::new());
        };
        let Some(env) = env.as_table() else {
            return Err(DefError::EnvNotStrings { location });
        };
        let mut out = BTreeMap::new();
        for (key, value) in env {
            let Some(value) = value.as_str() else {
                return Err(DefError::EnvNotStrings { location });
            };
            out.insert(key.clone(), value.to_owned());
        }
        Ok(out)
    }

    /// The common env with the substrate overlay applied (overlay wins).
    pub fn effective_env(
        &self,
        service_name: &str,
        substrate: &str,
    ) -> Result<BTreeMap<String, String>, DefError> {
        let overlay = self.substrate_env(service_name, substrate)?;
        if overlay.is_empty() {
            return Ok(self.env.clone());
        }
        let mut env = self.env.clone();
        env.extend(overlay);
        Ok(env)
    }
}
