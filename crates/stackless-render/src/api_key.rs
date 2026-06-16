//! Render API key resolution (§4): env var, then resolved secrets, then a 0600
//! key file. Resolution is shared (`stackless_cloud::credential`); the env var
//! and key-file names are Render's, and a miss maps to Render's error so its
//! `render.api_key.missing` code and remediation hold.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::RenderError;

pub const KEY_FILE: &str = ".render-api-key";
pub const KEY_ENV: &str = "RENDER_API_KEY";

pub fn resolve(
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, RenderError> {
    stackless_cloud::credential::resolve(KEY_ENV, KEY_FILE, definition_dir, secrets).map_err(
        |missing| RenderError::ApiKeyMissing {
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
        let key = resolve(dir.path(), &secret(KEY_ENV, "rnd_from_secrets")).unwrap();
        assert_eq!(key, "rnd_from_secrets");
    }

    #[test]
    fn key_file_is_a_fallback_when_secret_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "rnd_from_file\n").unwrap();
        let key = resolve(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(key, "rnd_from_file");
    }

    #[test]
    fn missing_everywhere_is_a_clean_fault() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, RenderError::ApiKeyMissing { .. }));
    }
}
