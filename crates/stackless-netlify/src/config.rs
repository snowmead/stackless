//! Parsing the netlify-specific blocks of the definition (§1 schema).
//!
//! Two deploy paths:
//! - **Static upload** (default when `build` is absent): clone the pinned ref
//!   and file-digest upload under `root` (or the repo root). Explicit fast path
//!   for pure static roots.
//! - **Build** (when `build` is set): Vercel-shaped build settings
//!   (`build` / `install` / `root` / `publish`) plus either zip-upload to
//!   Netlify's build API (`deploy = "build"`, default) or a git-linked site
//!   build (`deploy = "git"`).

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::NetlifyError;
use stackless_stripe_projects::CatalogService;

/// How source reaches Netlify for a build-path deploy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetlifyDeployMode {
    /// Zip the pinned checkout and POST it to Netlify's build API (no
    /// Netlify↔GitHub connection required).
    #[default]
    Build,
    /// Link the GitHub repo and trigger `createSiteBuild` (requires the
    /// Stripe-managed Netlify account be connected to GitHub).
    Git,
    /// File-digest static upload (no Netlify build step).
    Upload,
}

impl NetlifyDeployMode {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "build" => Some(Self::Build),
            "git" => Some(Self::Git),
            "upload" => Some(Self::Upload),
            _ => None,
        }
    }
}

/// A service's `[services.X.netlify]` block. Optional — an absent block uploads
/// the repo root as static files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceNetlify {
    /// Subdirectory of the cloned source used as the publish/base root.
    /// Static upload: the directory to upload. Build: the base directory.
    pub root: Option<String>,
    /// Build command (presence selects the build path unless `deploy = "upload"`).
    pub build: Option<String>,
    /// Optional install command prepended to `build` for the Netlify `cmd`.
    pub install: Option<String>,
    /// Publish directory after the build (Netlify `dir`). Defaults to `root`
    /// when unset on the build path.
    pub publish: Option<String>,
    pub deploy: NetlifyDeployMode,
}

impl ServiceNetlify {
    /// Whether this service uses Netlify's build pipeline (vs static upload).
    pub fn uses_build(&self) -> bool {
        match self.deploy {
            NetlifyDeployMode::Upload => false,
            NetlifyDeployMode::Build | NetlifyDeployMode::Git => self.build.is_some(),
        }
    }

    /// Effective Netlify build command (`install && build` when install is set).
    pub fn build_cmd(&self) -> Option<String> {
        let build = self.build.as_deref()?;
        match self.install.as_deref() {
            Some(install) => Some(format!("{install} && {build}")),
            None => Some(build.to_owned()),
        }
    }
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
            detail: "must be a table { root?, build?, install?, publish?, deploy?, env? }".into(),
        })?;
    for key in table.keys() {
        if !matches!(
            key.as_str(),
            "root" | "build" | "install" | "publish" | "deploy" | "env"
        ) {
            return Err(NetlifyError::ConfigInvalid {
                location: location.clone(),
                detail: format!(
                    "unknown key {key:?} (known: root, build, install, publish, deploy, env)"
                ),
            });
        }
    }
    let root = optional_str(table, "root", &location)?;
    let build = optional_str(table, "build", &location)?;
    let install = optional_str(table, "install", &location)?;
    let publish = optional_str(table, "publish", &location)?;
    let deploy = match table.get("deploy") {
        None => {
            if build.is_some() {
                NetlifyDeployMode::Build
            } else {
                NetlifyDeployMode::Upload
            }
        }
        Some(value) => {
            let raw = value.as_str().ok_or_else(|| NetlifyError::ConfigInvalid {
                location: format!("{location}.deploy"),
                detail: "must be a string".into(),
            })?;
            NetlifyDeployMode::parse(raw).ok_or_else(|| NetlifyError::ConfigInvalid {
                location: format!("{location}.deploy"),
                detail: format!("unknown deploy {raw:?} (known: build, git, upload)"),
            })?
        }
    };
    if matches!(deploy, NetlifyDeployMode::Build | NetlifyDeployMode::Git) && build.is_none() {
        return Err(NetlifyError::ConfigInvalid {
            location: format!("{location}.build"),
            detail: format!("`deploy = \"{}\"` requires `build`", deploy_str(deploy)),
        });
    }
    if install.is_some() && build.is_none() {
        return Err(NetlifyError::ConfigInvalid {
            location: format!("{location}.install"),
            detail: "`install` requires `build`".into(),
        });
    }
    Ok(ServiceNetlify {
        root,
        build,
        install,
        publish,
        deploy,
    })
}

fn deploy_str(mode: NetlifyDeployMode) -> &'static str {
    match mode {
        NetlifyDeployMode::Build => "build",
        NetlifyDeployMode::Git => "git",
        NetlifyDeployMode::Upload => "upload",
    }
}

fn optional_str(
    table: &toml::Table,
    key: &str,
    location: &str,
) -> Result<Option<String>, NetlifyError> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => {
            let Some(text) = value.as_str() else {
                return Err(NetlifyError::ConfigInvalid {
                    location: format!("{location}.{key}"),
                    detail: "must be a string".into(),
                });
            };
            if text.trim().is_empty() {
                return Err(NetlifyError::ConfigInvalid {
                    location: format!("{location}.{key}"),
                    detail: "must not be empty".into(),
                });
            }
            Ok(Some(text.to_owned()))
        }
    }
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

/// Parse `https://github.com/{org}/{repo}` for git-linked deploys.
pub fn parse_github_repo(url: &str) -> Result<(String, String), NetlifyError> {
    let trimmed = url.trim().trim_end_matches('/');
    let rest = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("https://www.github.com/"))
        .ok_or_else(|| NetlifyError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!("deploy = \"git\" requires a GitHub HTTPS remote (got {url:?})"),
        })?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let org = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if org.is_empty() || repo.is_empty() || parts.next().is_some() {
        return Err(NetlifyError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!("expected https://github.com/org/repo (got {url:?})"),
        });
    }
    Ok((org.to_owned(), repo.to_owned()))
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
    fn parses_static_root_and_defaults_when_block_absent() {
        let def = parse(BASE);
        let cfg = service_netlify(&def, "web").unwrap();
        assert_eq!(cfg.root.as_deref(), Some("fixtures/smoke/site"));
        assert_eq!(cfg.deploy, NetlifyDeployMode::Upload);
        assert!(!cfg.uses_build());
        let no_block = parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        );
        assert_eq!(
            service_netlify(&no_block, "web").unwrap(),
            ServiceNetlify::default()
        );
    }

    #[test]
    fn build_settings_select_build_path() {
        let def = parse(
            r#"
[stack]
name = "atto"
[services.web]
source = { repo = "https://github.com/snowmead/stackless", ref = "main" }
env = {}
health = { path = "/" }
[services.web.netlify]
root = "fixtures/smoke/site-build"
build = "mkdir -p dist && cp static/index.html dist/index.html"
publish = "dist"
"#,
        );
        let cfg = service_netlify(&def, "web").unwrap();
        assert!(cfg.uses_build());
        assert_eq!(cfg.deploy, NetlifyDeployMode::Build);
        assert_eq!(
            cfg.build_cmd().as_deref(),
            Some("mkdir -p dist && cp static/index.html dist/index.html")
        );
        assert_eq!(cfg.publish.as_deref(), Some("dist"));
    }

    #[test]
    fn install_prepends_build_cmd() {
        let def = parse(
            r#"
[stack]
name = "atto"
[services.web]
source = { repo = "https://github.com/acme/web", ref = "main" }
env = {}
health = { path = "/" }
[services.web.netlify]
build = "npm run build"
install = "npm ci"
publish = "dist"
deploy = "git"
"#,
        );
        let cfg = service_netlify(&def, "web").unwrap();
        assert_eq!(cfg.deploy, NetlifyDeployMode::Git);
        assert_eq!(cfg.build_cmd().as_deref(), Some("npm ci && npm run build"));
    }

    #[test]
    fn deploy_git_without_build_rejected() {
        let def = parse(
            r#"
[stack]
name = "atto"
[services.web]
source = { repo = "https://github.com/acme/web", ref = "main" }
env = {}
health = { path = "/" }
[services.web.netlify]
deploy = "git"
"#,
        );
        let err = service_netlify(&def, "web").unwrap_err();
        assert!(err.to_string().contains("requires `build`"));
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
    fn parse_github_repo_accepts_https() {
        assert_eq!(
            parse_github_repo("https://github.com/acme/widget.git").unwrap(),
            ("acme".into(), "widget".into())
        );
        assert!(parse_github_repo("https://gitlab.com/a/b").is_err());
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
