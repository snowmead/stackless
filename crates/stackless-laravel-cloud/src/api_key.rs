//! Laravel Cloud API token resolution: Stripe instance env, then env var /
//! resolved secrets, then a scoped key file.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::LaravelCloudError;

pub const KEY_FILE: &str = ".laravel-cloud-token";
pub const KEY_ENV: &str = "LARAVEL_CLOUD_API_TOKEN";

pub fn resolve(
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, LaravelCloudError> {
    stackless_cloud::credential::resolve(KEY_ENV, KEY_FILE, definition_dir, secrets).map_err(
        |missing| LaravelCloudError::ApiKeyMissing {
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
    fn resolves_from_stackless_env_secret() {
        let dir = tempfile::tempdir().unwrap();
        let key = resolve(dir.path(), &secret(KEY_ENV, "lc_from_secrets")).unwrap();
        assert_eq!(key, "lc_from_secrets");
    }

    #[test]
    fn key_file_is_a_fallback_when_secret_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "lc_from_file\n").unwrap();
        let key = resolve(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(key, "lc_from_file");
    }

    #[test]
    fn missing_everywhere_is_a_clean_fault() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, LaravelCloudError::ApiKeyMissing { .. }));
    }
}
