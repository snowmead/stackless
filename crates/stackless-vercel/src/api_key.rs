//! Vercel API token resolution.
//!
//! Order: the `VERCEL_TOKEN` env var, then a `VERCEL_TOKEN` entry in the
//! resolved secrets (`.stackless.env`), then a 0600 key file at
//! `<definition_dir>/.vercel-token`. A missing token is a clean fault naming
//! all three sources.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::VercelError;

pub const KEY_FILE: &str = ".vercel-token";
pub const KEY_ENV: &str = "VERCEL_TOKEN";

/// Resolve the token from the environment, the resolved secrets, or the scoped
/// key file next to the definition. The env var wins so CI can inject without a
/// file; `.stackless.env` is the project's canonical secret store; the file is a
/// scoped fallback.
pub fn resolve(
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, VercelError> {
    if let Ok(token) = std::env::var(KEY_ENV) {
        let token = token.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    if let Some(token) = secrets.get(KEY_ENV) {
        let token = token.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let key_file = definition_dir.join(KEY_FILE);
    if let Ok(contents) = std::fs::read_to_string(&key_file) {
        let token = contents.trim().to_owned();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    Err(VercelError::ApiKeyMissing {
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
        let token = resolve(dir.path(), &secret(KEY_ENV, "vc_from_secrets")).unwrap();
        assert_eq!(token, "vc_from_secrets");
    }

    #[test]
    fn key_file_is_a_fallback_when_secret_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "vc_from_file\n").unwrap();
        let token = resolve(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(token, "vc_from_file");
    }

    #[test]
    fn missing_everywhere_is_a_clean_fault() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, VercelError::ApiKeyMissing { .. }));
    }
}
