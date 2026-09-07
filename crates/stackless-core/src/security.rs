//! Public secret references, controlled child environments, and output redaction.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};

use serde::{Deserialize, Serialize};

/// A selector for controller-side injection. It contains no credential value.
/// The immutable owner must match the target workload before it is resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    kind: SecretRefKind,
    pub instance_id: String,
    pub integration: String,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum SecretRefKind {
    #[serde(rename = "secret_ref")]
    Reference,
}

impl SecretRef {
    pub fn new(instance_id: &str, integration: &str, output: &str) -> Self {
        Self {
            kind: SecretRefKind::Reference,
            instance_id: instance_id.into(),
            integration: integration.into(),
            output: output.into(),
        }
    }
}

/// These credentials authorize infrastructure operations. They are not an app's
/// secret namespace, even when a definition requests them under another name.
pub fn controller_credential(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "STRIPE_API_KEY"
            | "STRIPE_SECRET_KEY"
            | "STRIPE_API_TOKEN"
            | "RENDER_API_KEY"
            | "VERCEL_TOKEN"
            | "VERCEL_API_TOKEN"
            | "FLY_API_TOKEN"
            | "FLY_ACCESS_TOKEN"
            | "NETLIFY_AUTH_TOKEN"
            | "NETLIFY_ACCESS_TOKEN"
            | "RAILWAY_TOKEN"
            | "RAILWAY_API_TOKEN"
            | "CLOUDFLARE_API_TOKEN"
            | "CLOUDFLARE_API_KEY"
            | "CF_API_TOKEN"
            | "LARAVEL_CLOUD_API_TOKEN"
            | "GITLAB_TOKEN"
            | "GITLAB_ACCESS_TOKEN"
            | "WORDPRESS_ACCESS_TOKEN"
            | "WORDPRESS_API_TOKEN"
            | "STACKLESS_STATE_TOKEN"
            | "STACKLESS_CONTROLLER_TOKEN"
            | "AWS_ACCESS_KEY_ID"
            | "AWS_SECRET_ACCESS_KEY"
            | "AWS_SESSION_TOKEN"
            | "GOOGLE_APPLICATION_CREDENTIALS"
            | "AZURE_CLIENT_SECRET"
    )
}

pub fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "secret",
        "password",
        "token",
        "credential",
        "authorization",
        "cookie",
        "private_key",
        "api_key",
        "connection_string",
    ]
    .iter()
    .any(|part| key.contains(part))
        || key.ends_with("_dsn")
        || key.ends_with("database_url")
}

pub fn controller_values(secrets: &BTreeMap<String, String>) -> BTreeSet<String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .chain(secrets.iter().map(|(k, v)| (k.clone(), v.clone())))
        .filter(|(key, value)| controller_credential(key) && !value.is_empty())
        .map(|(_, value)| value)
        .collect()
}

pub fn application_secrets(secrets: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let privileged = controller_values(secrets);
    secrets
        .iter()
        .filter(|(key, value)| {
            !controller_credential(key) && !privileged.iter().any(|secret| value.contains(secret))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Call after interpolation too: literal values and renamed aliases do not
/// grant a workload access to a controller credential.
pub fn validate_environment<'a>(
    env: impl IntoIterator<Item = (&'a str, &'a str)>,
    secrets: &BTreeMap<String, String>,
) -> Result<(), String> {
    let privileged = controller_values(secrets);
    if env.into_iter().any(|(key, value)| {
        controller_credential(key) || privileged.iter().any(|secret| value.contains(secret))
    }) {
        return Err("workload environment requests an infrastructure credential".into());
    }
    Ok(())
}

/// Logs may contain raw app output on disk. Restrict access before writing.
pub fn private_log(path: &std::path::Path, append: bool) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

const CHILD_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TERM",
    "COLORTERM",
    "TZ",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "MISE_DATA_DIR",
    "MISE_CACHE_DIR",
    "SYSTEMROOT",
    "COMSPEC",
    "PATHEXT",
];

fn allowed_child_key(key: &OsStr) -> bool {
    key.to_str().is_some_and(|key| CHILD_ENV.contains(&key))
}

/// Call `env_clear` before applying this list and explicitly resolved app env.
pub fn child_environment() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| allowed_child_key(key))
        .collect()
}

#[derive(Clone, Default)]
pub struct Redactor {
    values: BTreeSet<String>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("value_count", &self.values.len())
            .finish()
    }
}

impl Redactor {
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        let mut redactor = Self::default();
        for value in values {
            redactor.register(&value);
        }
        redactor
    }

    pub fn register(&mut self, value: &str) {
        if value.is_empty() || value == "[redacted]" {
            return;
        }
        self.values.insert(value.into());
        // Provider credential blobs and connection URLs can be logged by field.
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(value) {
            self.register_json_leaves(&json);
        }
        if let Some((_, rest)) = value.split_once("://") {
            let authority = rest.split('/').next().unwrap_or_default();
            if let Some((userinfo, _)) = authority.rsplit_once('@')
                && let Some((_, password)) = userinfo.split_once(':')
                && !password.is_empty()
            {
                self.values.insert(password.into());
            }
        }
    }

    fn register_json_leaves(&mut self, value: &serde_json::Value) {
        match value {
            serde_json::Value::String(value) => {
                if !value.is_empty() {
                    self.values.insert(value.clone());
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    self.register_json_leaves(value);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    self.register_json_leaves(value);
                }
            }
            _ => {}
        }
    }

    pub fn text(&self, text: &str) -> String {
        let mut values: Vec<_> = self.values.iter().collect();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.into_iter().fold(text.into(), |text: String, value| {
            text.replace(value, "[redacted]")
        })
    }

    /// Metadata fields retain their protocol meaning. Free text and sensitive
    /// provider fields are redacted before entering a public envelope.
    pub fn value(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => *text = self.text(text),
            serde_json::Value::Array(values) => {
                for value in values {
                    self.value(value);
                }
            }
            serde_json::Value::Object(values) => {
                if serde_json::from_value::<SecretRef>(serde_json::Value::Object(values.clone()))
                    .is_ok()
                {
                    return;
                }
                for (key, value) in values {
                    if sensitive_key(key)
                        && serde_json::from_value::<SecretRef>(value.clone()).is_err()
                    {
                        *value = serde_json::Value::String("[redacted]".into());
                    } else if !matches!(
                        key.as_str(),
                        "code"
                            | "status"
                            | "verb"
                            | "id"
                            | "instance_id"
                            | "instance"
                            | "step_kind"
                            | "schema_version"
                    ) {
                        self.value(value);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The application cannot authorize host execution by declaring configuration.
pub fn host_grant_required() -> crate::substrate::SubstrateFault {
    crate::substrate::SubstrateFault {
        code: "execution.host_grant_required".into(),
        message: "host execution requires an explicit caller grant".into(),
        remediation: "use a container image or pass --allow-host-execution for code you trust with the controller host".into(),
        context: Box::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_environment_excludes_credentials_and_code_injection() {
        for key in [
            "STRIPE_API_KEY",
            "AWS_SESSION_TOKEN",
            "SSH_AUTH_SOCK",
            "NODE_OPTIONS",
            "PYTHONPATH",
            "BASH_ENV",
            "DYLD_INSERT_LIBRARIES",
        ] {
            assert!(!allowed_child_key(OsStr::new(key)));
        }
        assert!(allowed_child_key(OsStr::new("PATH")));
    }

    #[test]
    fn redacts_credential_fields_and_values_without_corrupting_handles_or_error_codes() {
        let redactor = Redactor::new([
            "secret-canary".into(),
            "postgres://user:database-password@db/one".into(),
        ]);
        let reference = SecretRef::new("owner", "database", "password");
        let mut value = serde_json::json!({"code":"state.query_failed","message":"secret-canary / database-password","token":"unseen-token","password":reference});
        redactor.value(&mut value);
        assert_eq!(value["message"], "[redacted] / [redacted]");
        assert_eq!(value["token"], "[redacted]");
        assert_eq!(value["password"]["kind"], "secret_ref");
        assert_eq!(value["code"], "state.query_failed");
        assert!(!format!("{redactor:?}").contains("secret-canary"));
    }
}
