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

/// `stripe projects env list --json` data. Tolerates the three shapes the CLI
/// has emitted: `{environments: {<name>: …}}`, `{environments: [{name}]}`, and a
/// bare `[{name}]` array.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn services_list_contains() {
        let r: ServicesListResponse =
            serde_json::from_value(json!({"services": [{"name": "atto-web"}]})).unwrap();
        assert!(r.contains("atto-web"));
        assert!(!r.contains("missing"));
    }
}
