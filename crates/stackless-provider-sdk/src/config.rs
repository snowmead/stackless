use std::collections::BTreeMap;

use crate::error::IntegrationError;

pub fn config_string(
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<String, IntegrationError> {
    config
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| IntegrationError::ConfigInvalid {
            location: format!("integrations.*.{key}"),
            detail: format!("{key} is required"),
        })
}

pub fn config_bool(config: &BTreeMap<String, toml::Value>, key: &str) -> bool {
    config
        .get(key)
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

pub fn config_optional_string(config: &BTreeMap<String, toml::Value>, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
}
