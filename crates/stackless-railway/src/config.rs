//! Parsing the railway-specific blocks of the definition (§1 schema).
//!
//! Phase 1 is Stripe-only: the substrate provisions `railway/hosting` and records
//! a best-effort origin. Deploying source to Railway (GraphQL/REST) is deferred.

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::RailwayError;
use stackless_stripe_projects::CatalogService;

/// A service's `[services.X.railway]` block. Optional — an absent block is valid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceRailway {}

/// The typed `railway/hosting` `--config`. Empty object is the catalog contract.
#[derive(Debug, Serialize)]
pub struct RailwayHostingConfig {}

impl CatalogService for RailwayHostingConfig {
    const REFERENCE: &'static str = "railway/hosting";
}

/// Read and shape-check `[services.<service>.railway]` (optional block; unknown
/// keys inside it are a fault, to trap agent typos).
pub fn service_railway(def: &StackDef, service: &str) -> Result<ServiceRailway, RailwayError> {
    let location = format!("services.{service}.railway");
    let Some(block) = def
        .services
        .get(service)
        .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
    else {
        return Ok(ServiceRailway::default());
    };
    let table = block
        .as_table()
        .ok_or_else(|| RailwayError::ConfigInvalid {
            location: location.clone(),
            detail: "must be a table { env? }".into(),
        })?;
    for key in table.keys() {
        if key.as_str() != "env" {
            return Err(RailwayError::ConfigInvalid {
                location: location.clone(),
                detail: format!("unknown key {key:?} (known: env)"),
            });
        }
    }
    Ok(ServiceRailway::default())
}

/// Whether `name` is a legal Railway service label: lowercase letter then
/// 2..=62 of `[a-z0-9-]` (DNS-safe, matches the cloud name rule).
pub fn is_valid_service_name(name: &str) -> bool {
    let len = name.len();
    if !(3..=63).contains(&len) {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
source = { repo = "https://github.com/snowmead/stackless", ref = "main" }
env = {}
health = { path = "/", contains = "ok" }
[services.web.railway]
"#;

    #[test]
    fn defaults_when_block_absent() {
        let def = parse(BASE);
        assert_eq!(
            service_railway(&def, "web").unwrap(),
            ServiceRailway::default()
        );
        let no_block = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        assert_eq!(
            service_railway(&no_block, "web").unwrap(),
            ServiceRailway::default()
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        let toml = BASE.to_owned() + "bogus = 1\n";
        let err = service_railway(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::RAILWAY_CONFIG_INVALID
        );
    }

    #[test]
    fn service_name_pattern() {
        assert!(is_valid_service_name("atto-demo-web"));
        assert!(!is_valid_service_name("ab"));
        assert!(!is_valid_service_name("1abc"));
        assert!(!is_valid_service_name("Abc"));
        assert!(!is_valid_service_name(&"a".repeat(64)));
    }

    #[test]
    fn typed_config_carries_its_catalog_reference() {
        assert_eq!(RailwayHostingConfig::REFERENCE, "railway/hosting");
    }

    /// Catalog gap check: the `railway/hosting` config must validate against the
    /// committed catalog fixture. Fails loudly if Stripe drifts the schema.
    #[test]
    fn railway_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures =
            stackless_stripe_projects::verify_service(&catalog, &RailwayHostingConfig {});
        assert!(
            failures.is_empty(),
            "railway/hosting catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
