//! Parsing the wordpress-specific blocks of the definition (§1 schema).
//!
//! Phase 1 is Stripe-only: the substrate provisions `wordpress.com/site` and
//! records a best-effort origin. Deploying source to WordPress.com is deferred.

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::WordPressError;
use stackless_stripe_projects::CatalogService;

/// A service's `[services.X.wordpress]` block. Optional — an absent block uses
/// `plan = "free"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceWordpress {
    pub plan: String,
}

/// The typed `wordpress.com/site` `--config`.
#[derive(Debug, Serialize)]
pub struct WordPressComSiteConfig {
    pub plan: String,
}

impl CatalogService for WordPressComSiteConfig {
    const REFERENCE: &'static str = "wordpress.com/site";
}

/// Read and shape-check `[services.<service>.wordpress]` (optional block; unknown
/// keys inside it are a fault, to trap agent typos).
pub fn service_wordpress(
    def: &StackDef,
    service: &str,
) -> Result<ServiceWordpress, WordPressError> {
    let location = format!("services.{service}.wordpress");
    let Some(block) = def
        .services
        .get(service)
        .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
    else {
        return Ok(ServiceWordpress {
            plan: "free".into(),
        });
    };
    let table = block
        .as_table()
        .ok_or_else(|| WordPressError::ConfigInvalid {
            location: location.clone(),
            detail: "must be a table { plan?, env? }".into(),
        })?;
    for key in table.keys() {
        if !matches!(key.as_str(), "plan" | "env") {
            return Err(WordPressError::ConfigInvalid {
                location: location.clone(),
                detail: format!("unknown key {key:?} (known: plan, env)"),
            });
        }
    }
    let plan = match table.get("plan") {
        None => "free".to_owned(),
        Some(value) => {
            let plan = value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| WordPressError::ConfigInvalid {
                    location: format!("{location}.plan"),
                    detail: "must be a non-empty string".into(),
                })?;
            if !is_valid_plan(plan) {
                return Err(WordPressError::ConfigInvalid {
                    location: format!("{location}.plan"),
                    detail: format!(
                        "must be one of free, personal, premium, business, commerce; got {plan:?}"
                    ),
                });
            }
            plan.to_owned()
        }
    };
    Ok(ServiceWordpress { plan })
}

/// Catalog enum for `wordpress.com/site` plan.
pub fn is_valid_plan(plan: &str) -> bool {
    matches!(
        plan,
        "free" | "personal" | "premium" | "business" | "commerce"
    )
}

/// Whether `name` is a legal WordPress site slug label: a lowercase letter then
/// 2..=62 of `[a-z0-9-]` (DNS-safe, matches the cloud name rule).
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
[services.web.wordpress]
plan = "free"
"#;

    #[test]
    fn parses_plan_and_defaults_when_block_absent() {
        let def = parse(BASE);
        assert_eq!(service_wordpress(&def, "web").unwrap().plan, "free");
        let no_block = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        assert_eq!(service_wordpress(&no_block, "web").unwrap().plan, "free");
    }

    #[test]
    fn unknown_key_is_rejected() {
        let toml = BASE.to_owned() + "bogus = 1\n";
        let err = service_wordpress(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::WORDPRESS_CONFIG_INVALID
        );
    }

    #[test]
    fn invalid_plan_is_rejected() {
        let toml = BASE.replace("plan = \"free\"", "plan = \"enterprise\"");
        let err = service_wordpress(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::WORDPRESS_CONFIG_INVALID
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
        assert_eq!(WordPressComSiteConfig::REFERENCE, "wordpress.com/site");
    }

    /// Catalog gap check: the `wordpress.com/site` config must validate against the
    /// committed catalog fixture. Fails loudly if Stripe drifts the schema.
    #[test]
    fn wordpress_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &WordPressComSiteConfig {
                plan: "free".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "wordpress.com/site catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
