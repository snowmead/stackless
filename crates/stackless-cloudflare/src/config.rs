//! Parsing the cloudflare-specific blocks of the definition (§1 schema).
//!
//! Phase 1 is Stripe-only: the substrate provisions `cloudflare/workers` and
//! records a best-effort `*.workers.dev` origin. Deploying a service's source to
//! Workers (Wrangler/API) is deferred.
//!
//! **Not** the Cloudflare catalog integrations in `stackless-integrations`
//! (`cloudflare-r2`, `cloudflare-kv`, `cloudflare-workers` integration, etc.).

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::CloudflareHostError;
use stackless_stripe_projects::CatalogService;

/// A service's `[services.X.cloudflare]` block. Optional — an absent block is valid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceCloudflare {}

/// The typed `cloudflare/workers` `--config`. Empty object is the catalog contract.
#[derive(Debug, Serialize)]
pub struct CloudflareWorkersConfig {}

impl CatalogService for CloudflareWorkersConfig {
    const REFERENCE: &'static str = "cloudflare/workers";
}

/// Read and shape-check `[services.<service>.cloudflare]` (optional block; unknown
/// keys inside it are a fault, to trap agent typos).
pub fn service_cloudflare(
    def: &StackDef,
    service: &str,
) -> Result<ServiceCloudflare, CloudflareHostError> {
    let location = format!("services.{service}.cloudflare");
    let Some(block) = def
        .services
        .get(service)
        .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
    else {
        return Ok(ServiceCloudflare::default());
    };
    let table = block
        .as_table()
        .ok_or_else(|| CloudflareHostError::ConfigInvalid {
            location: location.clone(),
            detail: "must be a table { env? }".into(),
        })?;
    for key in table.keys() {
        if key.as_str() != "env" {
            return Err(CloudflareHostError::ConfigInvalid {
                location: location.clone(),
                detail: format!("unknown key {key:?} (known: env)"),
            });
        }
    }
    Ok(ServiceCloudflare::default())
}

/// Whether `name` is a legal Workers script label: lowercase letter then
/// 2..=62 of `[a-z0-9-]` (DNS-safe, matches the cloud name rule).
pub fn is_valid_worker_name(name: &str) -> bool {
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
[services.web.cloudflare]
"#;

    #[test]
    fn defaults_when_block_absent() {
        let def = parse(BASE);
        assert_eq!(
            service_cloudflare(&def, "web").unwrap(),
            ServiceCloudflare::default()
        );
        let no_block = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        assert_eq!(
            service_cloudflare(&no_block, "web").unwrap(),
            ServiceCloudflare::default()
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        let toml = BASE.to_owned() + "bogus = 1\n";
        let err = service_cloudflare(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::CLOUDFLARE_HOST_CONFIG_INVALID
        );
    }

    #[test]
    fn worker_name_pattern() {
        assert!(is_valid_worker_name("atto-demo-web"));
        assert!(!is_valid_worker_name("ab"));
        assert!(!is_valid_worker_name("1abc"));
        assert!(!is_valid_worker_name("Abc"));
        assert!(!is_valid_worker_name(&"a".repeat(64)));
    }

    #[test]
    fn typed_config_carries_its_catalog_reference() {
        assert_eq!(CloudflareWorkersConfig::REFERENCE, "cloudflare/workers");
    }

    /// Catalog gap check: the `cloudflare/workers` config must validate against the
    /// committed catalog fixture. Fails loudly if Stripe drifts the schema.
    #[test]
    fn cloudflare_workers_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures =
            stackless_stripe_projects::verify_service(&catalog, &CloudflareWorkersConfig {});
        assert!(
            failures.is_empty(),
            "cloudflare/workers catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
