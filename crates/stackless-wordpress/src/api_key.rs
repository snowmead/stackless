//! WordPress.com OAuth access token resolution: Stripe instance env (handled in
//! `lib.rs`), then env vars, resolved secrets, then a scoped key file.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::WordPressError;

pub const KEY_FILE: &str = ".wordpress-com-token";
pub const KEY_ENV: &str = "WORDPRESS_COM_ACCESS_TOKEN";
pub const KEY_ENV_ALT: &str = "WORDPRESS_ACCESS_TOKEN";

/// Resolve from operator secrets / env / key file (not Stripe instance env).
pub fn resolve(
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, WordPressError> {
    if let Ok(value) = std::env::var(KEY_ENV_ALT) {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    if let Some(value) = secrets.get(KEY_ENV_ALT) {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    stackless_cloud::credential::resolve(KEY_ENV, KEY_FILE, definition_dir, secrets).map_err(
        |missing| WordPressError::ApiKeyMissing {
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
    fn resolves_primary_from_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let token = resolve(dir.path(), &secret(KEY_ENV, "wp_from_secrets")).unwrap();
        assert_eq!(token, "wp_from_secrets");
    }

    #[test]
    fn alt_env_var_wins_over_key_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "from_file\n").unwrap();
        let token = resolve(dir.path(), &secret(KEY_ENV_ALT, "wp_alt")).unwrap();
        assert_eq!(token, "wp_alt");
    }

    #[test]
    fn key_file_is_a_fallback_when_secret_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "wp_from_file\n").unwrap();
        let token = resolve(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(token, "wp_from_file");
    }

    #[test]
    fn missing_everywhere_is_a_clean_fault() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, WordPressError::ApiKeyMissing { .. }));
    }
}
