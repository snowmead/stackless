//! Shared cloud credential resolution (§4): env var → resolved secrets →
//! scoped key file. Each cloud substrate names its own env var and key file and
//! maps the neutral [`CredentialMissing`] to its own fault, so per-provider
//! error codes and remediation stay distinct (ARCHITECTURE.md §2).

use std::collections::BTreeMap;
use std::path::Path;

/// No credential in any source. The provider maps this to its own
/// `ApiKeyMissing`-style fault (whose remediation names the provider).
#[derive(Debug, Clone)]
pub struct CredentialMissing {
    pub env_var: String,
    pub key_file: String,
}

/// Resolve a credential from the environment, the resolved secrets, or a scoped
/// key file next to the definition. The env var wins so CI can inject without a
/// file; the secrets map is the project's canonical store; the file is a scoped
/// fallback.
pub fn resolve(
    env_var: &str,
    key_file_name: &str,
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, CredentialMissing> {
    if let Ok(value) = std::env::var(env_var) {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    if let Some(value) = secrets.get(env_var) {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    let key_file = definition_dir.join(key_file_name);
    if let Ok(contents) = std::fs::read_to_string(&key_file) {
        let value = contents.trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    Err(CredentialMissing {
        env_var: env_var.to_owned(),
        key_file: key_file.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: &str = "STACKLESS_CLOUD_TEST_KEY";
    const FILE: &str = ".cloud-test-key";

    fn secret(key: &str, value: &str) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert(key.to_owned(), value.to_owned());
        map
    }

    #[test]
    fn resolves_from_secret() {
        let dir = tempfile::tempdir().unwrap();
        let key = resolve(ENV, FILE, dir.path(), &secret(ENV, "from_secrets")).unwrap();
        assert_eq!(key, "from_secrets");
    }

    #[test]
    fn key_file_is_a_fallback_when_secret_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "from_file\n").unwrap();
        let key = resolve(ENV, FILE, dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(key, "from_file");
    }

    #[test]
    fn missing_everywhere_names_both_sources() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(ENV, FILE, dir.path(), &BTreeMap::new()).unwrap_err();
        assert_eq!(err.env_var, ENV);
        assert!(err.key_file.ends_with(FILE));
    }
}
