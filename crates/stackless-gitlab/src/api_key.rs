//! GitLab API token resolution: `GITLAB_TOKEN` or `GITLAB_ACCESS_TOKEN` from
//! the environment, resolved secrets, then a scoped `.gitlab-token` key file.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::GitLabError;

pub const KEY_FILE: &str = ".gitlab-token";
pub const KEY_ENV: &str = "GITLAB_TOKEN";
pub const ALT_KEY_ENV: &str = "GITLAB_ACCESS_TOKEN";

fn from_env_or_secrets(key: &str, secrets: &BTreeMap<String, String>) -> Option<String> {
    if let Ok(value) = std::env::var(key) {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return Some(value);
        }
    }
    secrets.get(key).and_then(|value| {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    })
}

pub fn resolve(
    definition_dir: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<String, GitLabError> {
    if let Some(token) =
        from_env_or_secrets(KEY_ENV, secrets).or_else(|| from_env_or_secrets(ALT_KEY_ENV, secrets))
    {
        return Ok(token);
    }
    stackless_cloud::credential::resolve(KEY_ENV, KEY_FILE, definition_dir, secrets).map_err(
        |missing| GitLabError::ApiKeyMissing {
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
    fn resolves_gitlab_token_from_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let token = resolve(dir.path(), &secret(KEY_ENV, "gl_from_secrets")).unwrap();
        assert_eq!(token, "gl_from_secrets");
    }

    #[test]
    fn access_token_alias_from_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let token = resolve(dir.path(), &secret(ALT_KEY_ENV, "gl_access")).unwrap();
        assert_eq!(token, "gl_access");
    }

    #[test]
    fn key_file_is_a_fallback_when_secret_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), "gl_from_file\n").unwrap();
        let token = resolve(dir.path(), &BTreeMap::new()).unwrap();
        assert_eq!(token, "gl_from_file");
    }

    #[test]
    fn missing_everywhere_is_a_clean_fault() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, GitLabError::ApiKeyMissing { .. }));
    }
}
