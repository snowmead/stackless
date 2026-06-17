//! Parsing the netlify-specific blocks of the definition (§1 schema).
//!
//! v0 Netlify is **static upload**: the substrate clones the service's pinned
//! ref, uploads the files under `[services.X.netlify].root` (or the repo root)
//! via the Netlify file-digest deploy API, and serves them at
//! `https://<site>.netlify.app`. A build step (running a framework build before
//! upload) is a later enhancement.

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::NetlifyError;
use stackless_stripe_projects::CatalogService;

/// A service's `[services.X.netlify]` block. Optional — an absent block uploads
/// the repo root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceNetlify {
    /// Subdirectory of the cloned source to upload (the publish dir).
    pub root: Option<String>,
}

/// The typed `netlify/project` `--config`. `name` is the only schema property
/// and IS the catalog contract — the gap test pins it.
#[derive(Debug, Serialize)]
pub struct NetlifyProjectConfig {
    pub name: String,
}

impl CatalogService for NetlifyProjectConfig {
    const REFERENCE: &'static str = "netlify/project";
}

/// Read and shape-check `[services.<service>.netlify]` (optional block; unknown
/// keys inside it are a fault, to trap agent typos).
pub fn service_netlify(def: &StackDef, service: &str) -> Result<ServiceNetlify, NetlifyError> {
    let location = format!("services.{service}.netlify");
    let Some(block) = def
        .services
        .get(service)
        .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
    else {
        return Ok(ServiceNetlify::default());
    };
    let table = block
        .as_table()
        .ok_or_else(|| NetlifyError::ConfigInvalid {
            location: location.clone(),
            detail: "must be a table { root?, env? }".into(),
        })?;
    for key in table.keys() {
        if !matches!(key.as_str(), "root" | "env") {
            return Err(NetlifyError::ConfigInvalid {
                location: location.clone(),
                detail: format!("unknown key {key:?} (known: root, env)"),
            });
        }
    }
    let root = match table.get("root") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| NetlifyError::ConfigInvalid {
                    location: format!("{location}.root"),
                    detail: "must be a non-empty string".into(),
                })?
                .to_owned(),
        ),
    };
    Ok(ServiceNetlify { root })
}

/// Whether `name` is a legal Netlify site name / subdomain label: a lowercase
/// letter then 2..=62 of `[a-z0-9-]` (DNS-safe, matches the cloud name rule).
pub fn is_valid_site_name(name: &str) -> bool {
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
[services.web.netlify]
root = "fixtures/smoke/site"
"#;

    #[test]
    fn parses_root_and_defaults_when_block_absent() {
        let def = parse(BASE);
        assert_eq!(
            service_netlify(&def, "web").unwrap().root.as_deref(),
            Some("fixtures/smoke/site")
        );
        let no_block = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        assert_eq!(
            service_netlify(&no_block, "web").unwrap(),
            ServiceNetlify::default()
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        let toml = BASE.replace("root = \"fixtures/smoke/site\"", "bogus = 1");
        let err = service_netlify(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::NETLIFY_CONFIG_INVALID
        );
    }

    #[test]
    fn site_name_pattern() {
        assert!(is_valid_site_name("atto-demo-web"));
        assert!(!is_valid_site_name("ab"));
        assert!(!is_valid_site_name("1abc"));
        assert!(!is_valid_site_name("Abc"));
        assert!(!is_valid_site_name(&"a".repeat(64)));
    }

    #[test]
    fn typed_config_carries_its_catalog_reference() {
        assert_eq!(NetlifyProjectConfig::REFERENCE, "netlify/project");
    }

    /// Catalog gap check: the `netlify/project` config must validate against the
    /// committed catalog fixture. Fails loudly if Stripe drifts the schema.
    #[test]
    fn netlify_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &NetlifyProjectConfig {
                name: "atto-demo-web".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "netlify catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
