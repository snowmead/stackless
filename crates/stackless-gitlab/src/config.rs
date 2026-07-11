//! Parsing the gitlab-specific blocks of the definition (§1 schema).
//!
//! Phase 1 is Stripe-only: the substrate provisions `gitlab/project` and records
//! a best-effort origin. Deploying source to GitLab (Pages, CI pipeline trigger,
//! container registry push) via the REST API is deferred — see `lib.rs`.

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::GitLabError;
use stackless_stripe_projects::CatalogService;

/// A service's `[services.X.gitlab]` block. Optional — an absent block uses
/// defaults (`visibility = "private"`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceGitlab {
    /// Catalog `visibility` override (`private` or `public`).
    pub visibility: Option<String>,
}

/// The typed `gitlab/project` `--config`. `name` and `visibility` are the
/// catalog contract — the gap test pins them.
#[derive(Debug, Serialize)]
pub struct GitLabProjectConfig {
    pub name: String,
    pub visibility: String,
}

impl CatalogService for GitLabProjectConfig {
    const REFERENCE: &'static str = "gitlab/project";
}

/// Read and shape-check `[services.<service>.gitlab]` (optional block; unknown
/// keys inside it are a fault, to trap agent typos).
pub fn service_gitlab(def: &StackDef, service: &str) -> Result<ServiceGitlab, GitLabError> {
    let location = format!("services.{service}.gitlab");
    let Some(block) = def
        .services
        .get(service)
        .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
    else {
        return Ok(ServiceGitlab::default());
    };
    let table = block.as_table().ok_or_else(|| GitLabError::ConfigInvalid {
        location: location.clone(),
        detail: "must be a table { visibility?, env? }".into(),
    })?;
    for key in table.keys() {
        if !matches!(key.as_str(), "visibility" | "env") {
            return Err(GitLabError::ConfigInvalid {
                location: location.clone(),
                detail: format!("unknown key {key:?} (known: visibility, env)"),
            });
        }
    }
    let visibility = match table.get("visibility") {
        None => None,
        Some(value) => {
            let vis = value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| GitLabError::ConfigInvalid {
                    location: format!("{location}.visibility"),
                    detail: "must be a non-empty string".into(),
                })?;
            if !is_valid_visibility(vis) {
                return Err(GitLabError::ConfigInvalid {
                    location: format!("{location}.visibility"),
                    detail: format!("must be \"private\" or \"public\", got {vis:?}"),
                });
            }
            Some(vis.to_owned())
        }
    };
    Ok(ServiceGitlab { visibility })
}

/// Catalog enum for `gitlab/project` visibility.
pub fn is_valid_visibility(visibility: &str) -> bool {
    matches!(visibility, "private" | "public")
}

/// Whether `name` is a legal GitLab project path segment label: a lowercase
/// letter then 2..=62 of `[a-z0-9-]` (DNS-safe, matches the cloud name rule).
pub fn is_valid_project_name(name: &str) -> bool {
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
[services.web.gitlab]
visibility = "private"
"#;

    #[test]
    fn parses_visibility_and_defaults_when_block_absent() {
        let def = parse(BASE);
        assert_eq!(
            service_gitlab(&def, "web").unwrap().visibility.as_deref(),
            Some("private")
        );
        let no_block = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        assert_eq!(
            service_gitlab(&no_block, "web").unwrap(),
            ServiceGitlab::default()
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        let toml = BASE.replace("visibility = \"private\"", "bogus = 1");
        let err = service_gitlab(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::GITLAB_CONFIG_INVALID
        );
    }

    #[test]
    fn invalid_visibility_is_rejected() {
        let toml = BASE.replace("visibility = \"private\"", "visibility = \"internal\"");
        let err = service_gitlab(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::GITLAB_CONFIG_INVALID
        );
    }

    #[test]
    fn project_name_pattern() {
        assert!(is_valid_project_name("atto-demo-web"));
        assert!(!is_valid_project_name("ab"));
        assert!(!is_valid_project_name("1abc"));
        assert!(!is_valid_project_name("Abc"));
        assert!(!is_valid_project_name(&"a".repeat(64)));
    }

    #[test]
    fn typed_config_carries_its_catalog_reference() {
        assert_eq!(GitLabProjectConfig::REFERENCE, "gitlab/project");
    }

    /// Catalog gap check: the `gitlab/project` config must validate against the
    /// committed catalog fixture. Fails loudly if Stripe drifts the schema.
    #[test]
    fn gitlab_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &GitLabProjectConfig {
                name: "atto-demo-web".into(),
                visibility: "private".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "gitlab/project catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
