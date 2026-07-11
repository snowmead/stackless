//! The definition model: serde structs sized exactly to the schema in
//! ARCHITECTURE.md §1.
//!
//! A service is substrate-independent identity + wiring + health; how a
//! substrate runs it is nested per substrate and captured here as opaque
//! TOML (`substrates` maps). Core never interprets a substrate block
//! beyond two contracts that §1 fixes across all substrates: the block
//! must be a table, and an `env` key inside it overlays the common env.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::error::DefError;
use crate::types::{DnsName, HttpStatus};

/// Top level of `stackless.toml`. Unknown top-level sections are
/// rejected (an old binary cannot honor a section it does not know).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackDef {
    pub stack: Stack,
    #[serde(default)]
    pub secrets: SecretsSpec,
    #[serde(default)]
    pub integrations: BTreeMap<String, Integration>,
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectsSpec {
    pub stripe: Option<StripeProjectSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StripeProjectSpec {
    pub project: Option<String>,
}

/// The proof contract, run by `stackless verify` (ARCHITECTURE.md §7).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifySpec {
    pub run: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// `[stack.verify]` plus optional named tiers under `[stack.verify.tiers.<name>]`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyRoot {
    pub run: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub tiers: BTreeMap<String, VerifySpec>,
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
            }),
            Some(name) => self.tiers.get(name).cloned(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretsSpec {
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Integration {
    /// Catalog adapter (e.g. `clerk` → `clerk/auth`).
    pub provider: String,
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

#[derive(Debug, Deserialize)]
pub struct Service {
    pub source: Source,
    /// Runs once after the service's source is materialized.
    pub setup: Option<String>,
    /// Runs on every `up`, after dependencies are ready, before start.
    pub prepare: Option<String>,
    /// Secrets injected as same-named env vars; must be in `[secrets].required`.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Every service declares a health check (ARCHITECTURE.md §1).
    pub health: Health,
    /// At most one service per stack also claims `http://{instance}.localhost`.
    #[serde(default)]
    pub root_origin: bool,
    /// Per-substrate run config (`[services.X.local]`, `[services.X.render]`, ...).
    #[serde(flatten)]
    pub substrates: BTreeMap<String, toml::Value>,
}

/// Code sources are git references (ARCHITECTURE.md §1).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// `health = { path, status = 200, contains = "..." }` (ARCHITECTURE.md §7).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub path: String,
    #[serde(default = "default_health_status")]
    pub status: HttpStatus,
    pub contains: Option<String>,
}

fn default_health_status() -> HttpStatus {
    HttpStatus::OK
}

impl Service {
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
