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

/// `stripe projects services list --json` data.
///
/// The CLI returns both deployable **services** and companion **plans** in one
/// payload. Parent-plan ensure must consult `plans` — matching only `services`
/// misses orphan `clerk/hobby` rows and retries `projects add` into a 500.
#[derive(Debug, Default, Deserialize)]
pub struct ServicesListResponse {
    #[serde(default)]
    pub services: Vec<ServiceRef>,
    #[serde(default)]
    pub plans: Vec<ServiceRef>,
}

impl ServicesListResponse {
    /// Whether a resource with this exact local name exists (service or plan).
    pub fn contains(&self, name: &str) -> bool {
        self.iter().any(|r| r.name.as_deref() == Some(name))
    }

    /// Resolve an existing project resource to reuse.
    ///
    /// Prefers an exact local `--name` match in `services` or `plans`.
    /// Otherwise picks a row whose catalog `service_id` equals `catalog_id`
    /// (e.g. plan `clerk-plan` with `service_id: hobby`, or service
    /// `clerk-auth` with `service_id: auth`). Among `service_id` matches,
    /// prefer `name == catalog_id`, then names prefixed by `catalog_id`, then
    /// lexicographic order.
    pub fn resolve(&self, name: &str, catalog_id: &str) -> Option<&str> {
        if let Some(found) = self
            .iter()
            .find_map(|r| r.name.as_deref().filter(|n| *n == name))
        {
            return Some(found);
        }
        let mut candidates: Vec<&str> = self
            .iter()
            .filter(|r| r.service_id.as_deref() == Some(catalog_id))
            .filter_map(|r| r.name.as_deref())
            .collect();
        candidates.sort_by_key(|candidate| {
            let rank = if *candidate == catalog_id {
                0u8
            } else if candidate.starts_with(catalog_id) {
                1
            } else {
                2
            };
            (rank, *candidate)
        });
        candidates.first().copied()
    }

    fn iter(&self) -> impl Iterator<Item = &ServiceRef> {
        self.services.iter().chain(self.plans.iter())
    }
}

#[derive(Debug, Deserialize)]
pub struct ServiceRef {
    #[serde(default)]
    pub name: Option<String>,
    /// Catalog service id (`hobby`, `auth`, …), distinct from the local `--name`.
    #[serde(default)]
    pub service_id: Option<String>,
}

/// Extract preflight rows from a full `{ok, data, error}` envelope.
pub fn preflight_checks_from_envelope(raw: &str) -> Vec<PreflightCheck> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        ok: bool,
        #[serde(default)]
        data: Option<Value>,
        #[serde(default)]
        error: Option<PreflightError>,
    }
    #[derive(Deserialize)]
    struct PreflightError {
        #[serde(default)]
        details: Option<Value>,
    }
    let envelope: Envelope = match serde_json::from_str(raw) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let data = envelope.data.unwrap_or(Value::Null);
    let details = envelope.error.and_then(|e| e.details);
    preflight_checks_from_parts(&data, envelope.ok, details.as_ref())
}

/// Extract preflight rows from already-parsed Stripe result parts.
pub fn preflight_checks_from_parts(
    data: &Value,
    ok: bool,
    error_details: Option<&Value>,
) -> Vec<PreflightCheck> {
    if ok {
        return serde_json::from_value::<PreflightReady>(data.clone())
            .map(|ready| ready.preflight)
            .unwrap_or_default();
    }
    let Some(details) = error_details else {
        return Vec::new();
    };
    #[derive(Deserialize)]
    struct Details {
        #[serde(default)]
        preflight: Vec<PreflightCheck>,
    }
    serde_json::from_value::<Details>(details.clone())
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
        let start = ENV_LIST.find('{').expect("fixture contains JSON object");
        let envelope: Envelope = serde_json::from_str(&ENV_LIST[start..]).unwrap();
        assert!(envelope.data.contains("default"));
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

    #[test]
    fn services_list_contains_plans_by_name() {
        let r: ServicesListResponse = serde_json::from_value(json!({
            "services": [{"name": "clerk-auth", "service_id": "auth"}],
            "plans": [{"name": "hobby", "service_id": "hobby"}]
        }))
        .unwrap();
        assert!(r.contains("hobby"));
        assert!(r.contains("clerk-auth"));
        assert_eq!(r.resolve("hobby", "hobby"), Some("hobby"));
    }

    #[test]
    fn services_list_resolve_prefers_service_id_fallback() {
        let r: ServicesListResponse = serde_json::from_value(json!({
            "services": [],
            "plans": [
                {"name": "clerk-plan", "service_id": "hobby"},
                {"name": "hobby-2", "service_id": "hobby"}
            ]
        }))
        .unwrap();
        assert!(!r.contains("hobby"));
        assert_eq!(r.resolve("hobby", "hobby"), Some("hobby-2"));
    }

    #[test]
    fn services_list_resolve_prefers_exact_name_over_service_id() {
        let r: ServicesListResponse = serde_json::from_value(json!({
            "plans": [
                {"name": "clerk-plan", "service_id": "hobby"},
                {"name": "hobby", "service_id": "hobby"},
                {"name": "hobby-2", "service_id": "hobby"}
            ]
        }))
        .unwrap();
        assert_eq!(r.resolve("hobby", "hobby"), Some("hobby"));
    }

    #[test]
    fn services_list_resolve_deployable_by_catalog_service_id() {
        let r: ServicesListResponse = serde_json::from_value(json!({
            "services": [{"name": "clerk-auth", "service_id": "auth"}],
            "plans": [{"name": "hobby", "service_id": "hobby"}]
        }))
        .unwrap();
        assert_eq!(r.resolve("e2e-clerk", "auth"), Some("clerk-auth"));
    }
}
