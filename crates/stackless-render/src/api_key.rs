//! Render API key resolution (§4).
//!
//! Order: the `RENDER_API_KEY` env var, then a `RENDER_API_KEY` entry in the
//! resolved secrets (`.stackless.env`), then a 0600 key file at
//! `<definition_dir>/.render-api-key`. A missing key is a clean fault naming
//! all three sources.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::RenderError;

pub const KEY_FILE: &str = ".render-api-key";
pub const KEY_ENV: &str = "RENDER_API_KEY";

/// Resolve the key from the environment, the resolved secrets, or the scoped
/// key file next to the definition. The env var wins so CI can inject without a
/// file; `.stackless.env` is the project's canonical secret store; the file is a
/// scoped fallback.
pub fn resolve(
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, RenderError> {
    if let Ok(key) = std::env::var(KEY_ENV) {
        let key = key.trim().to_owned();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    if let Some(key) = secrets.get(KEY_ENV) {
        let key = key.trim().to_owned();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    let key_file = definition_dir.join(KEY_FILE);
    if let Ok(contents) = std::fs::read_to_string(&key_file) {
        let key = contents.trim().to_owned();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err(RenderError::ApiKeyMissing {
        key_file: key_file.display().to_string(),
    })
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
