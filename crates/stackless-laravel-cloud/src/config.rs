//! Parsing the laravel-cloud-specific blocks of the definition (§1 schema).

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::LaravelCloudError;
use stackless_stripe_projects::CatalogService;

/// A service's `[services.X.laravel-cloud]` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLaravelCloud {
    pub region: String,
    pub repository: String,
    pub create_cache: Option<String>,
    pub create_database: Option<String>,
}

/// The typed `laravel_cloud/application` `--config`.
#[derive(Debug, Serialize)]
pub struct LaravelCloudApplicationConfig {
    pub name: String,
    pub region: String,
    pub repository: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_cache: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_database: Option<String>,
}

impl CatalogService for LaravelCloudApplicationConfig {
    const REFERENCE: &'static str = "laravel_cloud/application";
}

/// Read and shape-check `[services.<service>.laravel-cloud]`.
pub fn service_laravel_cloud(
    def: &StackDef,
    service: &str,
) -> Result<ServiceLaravelCloud, LaravelCloudError> {
    let location = format!("services.{service}.laravel-cloud");
    let block = def
        .services
        .get(service)
        .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
        .ok_or_else(|| LaravelCloudError::ConfigInvalid {
            location: location.clone(),
            detail: "missing [services.X.laravel-cloud] block".into(),
        })?;
    let table = block
        .as_table()
        .ok_or_else(|| LaravelCloudError::ConfigInvalid {
            location: location.clone(),
            detail: "must be a table { region, repository, create_cache?, create_database?, env? }"
                .into(),
        })?;
    for key in table.keys() {
        if !matches!(
            key.as_str(),
            "region" | "repository" | "create_cache" | "create_database" | "env"
        ) {
            return Err(LaravelCloudError::ConfigInvalid {
                location: location.clone(),
                detail: format!(
                    "unknown key {key:?} (known: region, repository, create_cache, create_database, env)"
                ),
            });
        }
    }
    let region = required_string(table, &location, "region")?;
    let repository = required_string(table, &location, "repository")?;
    let create_cache = optional_string(table, &location, "create_cache")?;
    let create_database = optional_string(table, &location, "create_database")?;
    Ok(ServiceLaravelCloud {
        region,
        repository,
        create_cache,
        create_database,
    })
}

fn required_string(
    table: &toml::Table,
    location: &str,
    key: &str,
) -> Result<String, LaravelCloudError> {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| LaravelCloudError::ConfigInvalid {
            location: format!("{location}.{key}"),
            detail: "must be a non-empty string".into(),
        })
}

fn optional_string(
    table: &toml::Table,
    location: &str,
    key: &str,
) -> Result<Option<String>, LaravelCloudError> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => Ok(Some(
            value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| LaravelCloudError::ConfigInvalid {
                    location: format!("{location}.{key}"),
                    detail: "must be a non-empty string".into(),
                })?
                .to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> StackDef {
        StackDef::parse(toml).expect("valid base toml")
    }

    const BASE: &str = r#"
[stack]
name = "atto"
[services.web]
source = { repo = "https://github.com/laravel/cloud", ref = "main" }
env = {}
health = { path = "/", contains = "ok" }
[services.web.laravel-cloud]
region = "us-east-1"
repository = "laravel/cloud"
"#;

    #[test]
    fn parses_required_fields() {
        let def = parse(BASE);
        let cfg = service_laravel_cloud(&def, "web").unwrap();
        assert_eq!(cfg.region, "us-east-1");
        assert_eq!(cfg.repository, "laravel/cloud");
        assert_eq!(cfg.create_cache, None);
        assert_eq!(cfg.create_database, None);
    }

    #[test]
    fn missing_block_is_rejected() {
        let def = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        let err = service_laravel_cloud(&def, "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::LARAVEL_CLOUD_CONFIG_INVALID
        );
    }

    #[test]
    fn typed_config_carries_its_catalog_reference() {
        assert_eq!(
            LaravelCloudApplicationConfig::REFERENCE,
            "laravel_cloud/application"
        );
    }

    #[test]
    fn laravel_cloud_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &LaravelCloudApplicationConfig {
                name: "atto-demo-web".into(),
                region: "us-east-1".into(),
                repository: "laravel/cloud".into(),
                create_cache: None,
                create_database: None,
            },
        );
        assert!(
            failures.is_empty(),
            "laravel_cloud/application catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
