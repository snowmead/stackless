//! Railway API token resolution: env var, then resolved secrets, then a 0600 key
//! file. Resolution is shared (`stackless_cloud::credential`); the env var and
//! key-file names are Railway's, and a miss maps to Railway's error so its
//! `railway.api_key.missing` code and remediation hold.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::RailwayError;

pub const KEY_FILE: &str = ".railway-token";
pub const KEY_ENV: &str = "RAILWAY_TOKEN";

pub fn resolve(
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, RailwayError> {
    if let Some(token) = secrets
        .get("RAILWAY_API_TOKEN")
        .filter(|t| !t.trim().is_empty())
    {
        return Ok(token.clone());
    }
    stackless_cloud::credential::resolve(KEY_ENV, KEY_FILE, definition_dir, secrets).map_err(
        |missing| RailwayError::ApiKeyMissing {
            key_file: missing.key_file,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(key: &str, value: &str) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert(key.to_owned(), value.to_owned());
        map
    }

    #[test]
    fn resolves_railway_api_token_from_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let token = resolve(
            dir.path(),
            &secret("RAILWAY_API_TOKEN", "rw_from_api_token"),
        )
        .unwrap();
        assert_eq!(token, "rw_from_api_token");
    }

    #[test]
    fn resolves_from_stackless_env_secret() {
        let dir = tempfile::tempdir().unwrap();
        let token = resolve(dir.path(), &secret(KEY_ENV, "rw_from_secrets")).unwrap();
        assert_eq!(token, "rw_from_secrets");
    }

    #[test]
    fn key_file_is_a_fallback_when_secret_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "rw_from_file\n").unwrap();
        let token = resolve(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(token, "rw_from_file");
    }

    #[test]
    fn missing_everywhere_is_a_clean_fault() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, RailwayError::ApiKeyMissing { .. }));
    }
}
