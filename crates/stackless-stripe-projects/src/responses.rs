//! Typed envelopes for the `stripe projects` reads the driver depends on,
//! replacing string-keyed `serde_json::Value` traversal.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// `stripe projects status --json` data.
#[derive(Debug, Default, Deserialize)]
pub struct StatusResponse {
    #[serde(default)]
    pub project: Option<ProjectRef>,
}

impl StatusResponse {
    /// The linked project id, if any.
    pub fn project_id(&self) -> Option<&str> {
        self.project.as_ref().and_then(|p| p.id.as_deref())
    }
}

#[derive(Debug, Deserialize)]
pub struct ProjectRef {
    #[serde(default)]
    pub id: Option<String>,
}

/// One row from a `--preflight` response (`data.preflight` or
/// `error.details.preflight`).
#[derive(Debug, Clone, Deserialize)]
pub struct PreflightCheck {
    pub label: String,
    pub pass: bool,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub remedy: Option<String>,
}

/// Successful `--preflight` payload (`data` when `ok: true`).
#[derive(Debug, Default, Deserialize)]
pub struct PreflightReady {
    #[serde(default)]
    pub preflight: Vec<PreflightCheck>,
    #[serde(default)]
    pub ready: Option<bool>,
}

/// `stripe projects env list --json` data. Tolerates the three shapes the CLI
/// has emitted: `{environments: {<name>: …}}`, `{environments: [{name}]}`, and a
/// bare `[{name}]` array. At 0.23.0 the canonical shape is the named map inside
/// `data.environments`; older bare/list shapes are kept for fixture stability.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EnvListResponse {
    // `Bare` first: serde can deserialize a struct from a sequence, so `Wrapped`
    // would greedily swallow a bare array if it came first.
    Bare(Vec<EnvRef>),
    Wrapped(EnvWrapper),
}

#[derive(Debug, Deserialize)]
pub struct EnvWrapper {
    #[serde(default)]
    pub environments: Option<EnvCollection>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EnvCollection {
    Named(BTreeMap<String, Value>),
    List(Vec<EnvRef>),
}

#[derive(Debug, Deserialize)]
pub struct EnvRef {
    #[serde(default)]
    pub name: Option<String>,
}

impl EnvListResponse {
    /// Whether an environment named `instance` is present.
    pub fn contains(&self, instance: &str) -> bool {
        let in_list = |list: &[EnvRef]| list.iter().any(|e| e.name.as_deref() == Some(instance));
        match self {
            Self::Wrapped(wrapper) => match &wrapper.environments {
                Some(EnvCollection::Named(map)) => map.contains_key(instance),
                Some(EnvCollection::List(list)) => in_list(list),
                None => false,
            },
            Self::Bare(list) => in_list(list),
        }
    }
}

/// `stripe projects variables list --json` data. Values are never returned —
/// only bindings and `has_value` metadata (probe: fixtures/probes/variables-list.json).
#[derive(Debug, Default, Deserialize)]
pub struct VariablesListResponse {
    #[serde(default)]
    pub variables: Vec<ProjectVariable>,
}

#[derive(Debug, Deserialize)]
pub struct ProjectVariable {
    pub name: String,
    #[serde(default)]
    pub has_value: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub bindings: Vec<VariableBinding>,
}

#[derive(Debug, Deserialize)]
pub struct VariableBinding {
    pub env_key: String,
    #[serde(default)]
    pub environment: Option<String>,
}

impl VariablesListResponse {
    /// Map env keys (e.g. `PROBE_SECRET`) configured via project variables.
    pub fn env_keys(&self) -> Vec<&str> {
        self.variables
            .iter()
            .flat_map(|v| v.bindings.iter().map(|b| b.env_key.as_str()))
            .collect()
    }
}

/// `stripe projects services list --json` data.
#[derive(Debug, Default, Deserialize)]
pub struct ServicesListResponse {
    #[serde(default)]
    pub services: Vec<ServiceRef>,
}

impl ServicesListResponse {
    /// Whether a registered service named `name` exists.
    pub fn contains(&self, name: &str) -> bool {
        self.services
            .iter()
            .any(|s| s.name.as_deref() == Some(name))
    }
}

#[derive(Debug, Deserialize)]
pub struct ServiceRef {
    #[serde(default)]
    pub name: Option<String>,
}

/// Extract preflight rows from a full `{ok, data, error}` envelope.
pub fn preflight_checks_from_envelope(raw: &str) -> Vec<PreflightCheck> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        data: Option<PreflightReady>,
        #[serde(default)]
        error: Option<PreflightError>,
    }
    #[derive(Deserialize)]
    struct PreflightError {
        #[serde(default)]
        details: Option<PreflightDetails>,
    }
    #[derive(Deserialize)]
    struct PreflightDetails {
        #[serde(default)]
        preflight: Vec<PreflightCheck>,
    }
    let envelope: Envelope = match serde_json::from_str(raw) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    if let Some(data) = envelope.data {
        return data.preflight;
    }
    envelope
        .error
        .and_then(|e| e.details)
        .map(|d| d.preflight)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PREFLIGHT_INIT_BLOCKED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/probes/preflight-init-blocked.json"
    ));
    const PREFLIGHT_INIT_READY: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/probes/preflight-init-ready.json"
    ));
    const VARIABLES_LIST: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/probes/variables-list.json"
    ));
    const ENV_LIST: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/probes/env-list.json"
    ));

    #[test]
    fn status_reads_project_id() {
        let r: StatusResponse =
            serde_json::from_value(json!({"project": {"id": "proj_1"}})).unwrap();
        assert_eq!(r.project_id(), Some("proj_1"));
        let empty: StatusResponse = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty.project_id(), None);
    }

    #[test]
    fn env_list_handles_all_three_shapes() {
        let named: EnvListResponse =
            serde_json::from_value(json!({"environments": {"feat-x": {}}})).unwrap();
        assert!(named.contains("feat-x"));
        let listed: EnvListResponse =
            serde_json::from_value(json!({"environments": [{"name": "feat-x"}]})).unwrap();
        assert!(listed.contains("feat-x"));
        let bare: EnvListResponse = serde_json::from_value(json!([{"name": "feat-x"}])).unwrap();
        assert!(bare.contains("feat-x"));
        assert!(!bare.contains("other"));
    }

    #[test]
    fn env_list_parses_probe_fixture() {
        #[derive(Deserialize)]
        struct Envelope {
            data: EnvListResponse,
        }
        let start = ENV_LIST.find('{').unwrap();
        let envelope: Envelope = serde_json::from_str(&ENV_LIST[start..]).unwrap();
        assert!(envelope.data.contains("default"));
    }

    #[test]
    fn variables_list_parses_probe_fixture() {
        #[derive(Deserialize)]
        struct Envelope {
            data: VariablesListResponse,
        }
        let start = VARIABLES_LIST.find('{').unwrap();
        let envelope: Envelope = serde_json::from_str(&VARIABLES_LIST[start..]).unwrap();
        assert_eq!(envelope.data.variables.len(), 1);
        assert_eq!(envelope.data.env_keys(), vec!["PROBE_SECRET"]);
    }

    #[test]
    fn preflight_blocked_fixture_lists_failures() {
        let checks = preflight_checks_from_envelope(PREFLIGHT_INIT_BLOCKED);
        let failures: Vec<_> = checks.iter().filter(|c| !c.pass).collect();
        assert_eq!(failures.len(), 2);
        assert!(
            failures
                .iter()
                .any(|c| c.code.as_deref() == Some("TOS_ACCEPTANCE_REQUIRED"))
        );
    }

    #[test]
    fn preflight_ready_fixture_passes() {
        let checks = preflight_checks_from_envelope(PREFLIGHT_INIT_READY);
        assert!(checks.iter().all(|c| c.pass));
    }

    #[test]
    fn services_list_contains() {
        let r: ServicesListResponse =
            serde_json::from_value(json!({"services": [{"name": "atto-web"}]})).unwrap();
        assert!(r.contains("atto-web"));
        assert!(!r.contains("missing"));
    }
}
