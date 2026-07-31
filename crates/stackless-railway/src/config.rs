//! Parsing the railway-specific blocks of the definition (§1 schema).
//!
//! Two deploy paths:
//! - **Image** (`image = "..."`): deploy a prebuilt container via Railway GraphQL.
//! - **GitHub** (no `image`): link `source.repo` (GitHub HTTPS) and deploy the
//!   pinned branch.

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::RailwayError;
use stackless_stripe_projects::CatalogService;

/// How a Railway service reaches a runnable image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailwayDeployMode {
    Image {
        image: String,
        cmd: Option<Vec<String>>,
    },
    GitHub,
}

/// A service's `[services.X.railway]` block. Optional — an absent block uses
/// the GitHub path from `source.repo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRailway {
    pub mode: RailwayDeployMode,
}

impl Default for ServiceRailway {
    fn default() -> Self {
        Self {
            mode: RailwayDeployMode::GitHub,
        }
    }
}

impl ServiceRailway {
    pub fn image(&self) -> Option<&str> {
        match &self.mode {
            RailwayDeployMode::Image { image, .. } => Some(image.as_str()),
            RailwayDeployMode::GitHub => None,
        }
    }

    pub fn cmd(&self) -> Option<&[String]> {
        match &self.mode {
            RailwayDeployMode::Image { cmd, .. } => cmd.as_deref(),
            RailwayDeployMode::GitHub => None,
        }
    }
}

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
        return Ok(ServiceRailway {
            mode: RailwayDeployMode::GitHub,
        });
    };
    let table = block
        .as_table()
        .ok_or_else(|| RailwayError::ConfigInvalid {
            location: location.clone(),
            detail: "must be a table { image?, cmd?, env? }".into(),
        })?;
    for key in table.keys() {
        if !matches!(key.as_str(), "image" | "cmd" | "env") {
            return Err(RailwayError::ConfigInvalid {
                location: location.clone(),
                detail: format!("unknown key {key:?} (known: image, cmd, env)"),
            });
        }
    }
    let image = table
        .get("image")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let cmd = table.get("cmd").and_then(parse_cmd_array);
    if cmd.is_some() && image.is_none() {
        return Err(RailwayError::ConfigInvalid {
            location: location.clone(),
            detail: "`cmd` requires `image`".into(),
        });
    }
    let mode = match image {
        Some(image) => RailwayDeployMode::Image { image, cmd },
        None => RailwayDeployMode::GitHub,
    };
    Ok(ServiceRailway { mode })
}

fn parse_cmd_array(value: &toml::Value) -> Option<Vec<String>> {
    let arr = value.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(item.as_str()?.to_owned());
    }
    Some(out)
}

/// Parse `https://github.com/{org}/{repo}` for git-linked deploys.
pub fn parse_github_repo(url: &str) -> Result<(String, String), RailwayError> {
    let trimmed = url.trim().trim_end_matches('/');
    let rest = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("https://www.github.com/"))
        .ok_or_else(|| RailwayError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!("Railway git deploy requires a GitHub HTTPS remote (got {url:?})"),
        })?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let org = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if org.is_empty() || repo.is_empty() || parts.next().is_some() {
        return Err(RailwayError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!("expected https://github.com/org/repo (got {url:?})"),
        });
    }
    Ok((org.to_owned(), repo.to_owned()))
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
    fn defaults_to_github_when_block_empty() {
        let def = parse(BASE);
        assert_eq!(
            service_railway(&def, "web").unwrap().mode,
            RailwayDeployMode::GitHub
        );
        let no_block = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"https://github.com/a/b\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        assert_eq!(
            service_railway(&no_block, "web").unwrap().mode,
            RailwayDeployMode::GitHub
        );
    }

    #[test]
    fn image_and_cmd_parse() {
        let toml = BASE.to_owned()
            + "image = \"hashicorp/http-echo\"\ncmd = [\"-text=ok\", \"-listen=:8080\"]\n";
        let cfg = service_railway(&parse(&toml), "web").unwrap();
        assert_eq!(cfg.image(), Some("hashicorp/http-echo"));
        assert_eq!(
            cfg.cmd(),
            Some(["-text=ok".into(), "-listen=:8080".into()].as_slice())
        );
    }

    #[test]
    fn cmd_without_image_is_rejected() {
        let toml = BASE.to_owned() + "cmd = [\"x\"]\n";
        let err = service_railway(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::RAILWAY_CONFIG_INVALID
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
    fn parse_github_repo_accepts_https() {
        assert_eq!(
            parse_github_repo("https://github.com/acme/widget.git").unwrap(),
            ("acme".into(), "widget".into())
        );
        assert!(parse_github_repo("https://gitlab.com/a/b").is_err());
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
