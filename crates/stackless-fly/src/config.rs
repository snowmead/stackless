//! Parsing the fly-specific blocks of the definition (§1 schema).
//!
//! `validate_definition` checks these shapes strictly — unknown keys are a fault
//! (agent-trap protection, mirroring stackless-render). The same parsers feed
//! the Substrate impl so config is read in exactly one place.
//!
//! Two deploy paths:
//! - **Image** (`image = "..."`): explicit fast path — deploy a prebuilt container.
//! - **Source-build** (no `image`): clone the pinned ref and build via Fly's
//!   remote builder (`flyctl deploy --remote-only`), then run the resulting
//!   machine. Optional `dockerfile` (default `Dockerfile`).

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::FlyError;
use stackless_stripe_projects::CatalogService;

/// Default machine guest (Fly's smallest shared preset).
const DEFAULT_CPU_KIND: &str = "shared";
const DEFAULT_CPUS: i64 = 1;
const DEFAULT_MEMORY_MB: i64 = 256;
/// Default port the container listens on if `[services.X.fly].internal_port` is
/// omitted (Fly's own convention for autodetected web services).
const DEFAULT_INTERNAL_PORT: i64 = 8080;
const DEFAULT_DOCKERFILE: &str = "Dockerfile";

/// How a Fly service reaches a runnable image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlyDeployMode {
    /// Prebuilt container image (explicit fast path).
    Image { image: String },
    /// Build from the pinned checkout via Fly remote builder.
    Build { dockerfile: String },
}

/// A service's `[services.X.fly]` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFly {
    pub mode: FlyDeployMode,
    pub internal_port: u16,
    /// Overrides the image entrypoint/cmd (Fly `config.init.cmd` / container
    /// args), e.g. `["-text=ok", "-listen=:5678"]`. Image path only.
    pub cmd: Option<Vec<String>>,
    pub guest: FlyGuest,
}

impl ServiceFly {
    /// Prebuilt image ref when `mode` is [`FlyDeployMode::Image`].
    pub fn image(&self) -> Option<&str> {
        match &self.mode {
            FlyDeployMode::Image { image } => Some(image.as_str()),
            FlyDeployMode::Build { .. } => None,
        }
    }

    /// Dockerfile path (repo-relative) when `mode` is [`FlyDeployMode::Build`].
    pub fn dockerfile(&self) -> Option<&str> {
        match &self.mode {
            FlyDeployMode::Build { dockerfile } => Some(dockerfile.as_str()),
            FlyDeployMode::Image { .. } => None,
        }
    }
}

/// The Fly machine guest (CPU/memory preset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlyGuest {
    pub cpu_kind: String,
    pub cpus: u32,
    pub memory_mb: u32,
}

impl Default for FlyGuest {
    fn default() -> Self {
        Self {
            cpu_kind: DEFAULT_CPU_KIND.to_owned(),
            cpus: DEFAULT_CPUS as u32,
            memory_mb: DEFAULT_MEMORY_MB as u32,
        }
    }
}

/// The typed `flyio/app` `--config`. `app_name` is the only schema property and
/// IS the catalog contract — the gap test pins it.
#[derive(Debug, Serialize)]
pub struct FlyAppConfig {
    pub app_name: String,
}

impl CatalogService for FlyAppConfig {
    const REFERENCE: &'static str = "flyio/app";
}

/// Read and shape-check `[services.<service>.fly]`.
pub fn service_fly(def: &StackDef, service: &str) -> Result<ServiceFly, FlyError> {
    let location = format!("services.{service}.fly");
    let block = def
        .services
        .get(service)
        .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
        .and_then(|value| value.as_table())
        .ok_or_else(|| FlyError::ConfigInvalid {
            location: location.clone(),
            detail: "missing [services.X.fly] block".into(),
        })?;

    for key in block.keys() {
        if !matches!(
            key.as_str(),
            "image"
                | "dockerfile"
                | "internal_port"
                | "cmd"
                | "env"
                | "cpu_kind"
                | "cpus"
                | "memory_mb"
        ) {
            return Err(FlyError::ConfigInvalid {
                location: location.clone(),
                detail: format!(
                    "unknown key {key:?} (known: image, dockerfile, internal_port, cmd, env, \
                     cpu_kind, cpus, memory_mb)"
                ),
            });
        }
    }

    let image = opt_str(block, "image");
    let dockerfile = opt_str(block, "dockerfile");
    let mode = match (image, dockerfile) {
        (Some(_image), Some(_)) => {
            return Err(FlyError::ConfigInvalid {
                location: location.clone(),
                detail: "set either `image` (prebuilt fast path) or `dockerfile` / omit both for \
                         source-build — not both"
                    .into(),
            });
        }
        (Some(image), None) => FlyDeployMode::Image { image },
        (None, dockerfile) => FlyDeployMode::Build {
            dockerfile: dockerfile.unwrap_or_else(|| DEFAULT_DOCKERFILE.to_owned()),
        },
    };

    let internal_port =
        u16::try_from(opt_int(block, "internal_port", &location)?.unwrap_or(DEFAULT_INTERNAL_PORT))
            .map_err(|_| FlyError::ConfigInvalid {
                location: format!("{location}.internal_port"),
                detail: "must be a TCP port in 1..=65535".into(),
            })?;
    if internal_port == 0 {
        return Err(FlyError::ConfigInvalid {
            location: format!("{location}.internal_port"),
            detail: "must be a TCP port in 1..=65535".into(),
        });
    }
    let cmd = opt_string_array(block, "cmd", &location)?;
    if cmd.is_some() && matches!(mode, FlyDeployMode::Build { .. }) {
        return Err(FlyError::ConfigInvalid {
            location: format!("{location}.cmd"),
            detail: "`cmd` is only supported with `image` (source-build uses the Dockerfile CMD)"
                .into(),
        });
    }
    let guest = FlyGuest {
        cpu_kind: opt_str(block, "cpu_kind").unwrap_or_else(|| DEFAULT_CPU_KIND.to_owned()),
        cpus: u32::try_from(opt_int(block, "cpus", &location)?.unwrap_or(DEFAULT_CPUS)).map_err(
            |_| FlyError::ConfigInvalid {
                location: format!("{location}.cpus"),
                detail: "must be a positive integer".into(),
            },
        )?,
        memory_mb: u32::try_from(
            opt_int(block, "memory_mb", &location)?.unwrap_or(DEFAULT_MEMORY_MB),
        )
        .map_err(|_| FlyError::ConfigInvalid {
            location: format!("{location}.memory_mb"),
            detail: "must be a positive integer".into(),
        })?,
    };

    Ok(ServiceFly {
        mode,
        internal_port,
        cmd,
        guest,
    })
}

/// The recorded `[stack.fly].region`, defaulting to `iad` (US-East).
pub fn stack_region(def: &StackDef) -> String {
    def.stack
        .substrates
        .get(SUBSTRATE_NAME)
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("region"))
        .and_then(|value| value.as_str())
        .unwrap_or("iad")
        .to_owned()
}

/// Whether `name` is a legal Fly app name (catalog pattern
/// `^[a-z][a-z0-9-]{2,62}$`): a lowercase letter then 2..=62 of `[a-z0-9-]`.
/// Checked without a regex dependency.
pub fn is_valid_app_name(name: &str) -> bool {
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

fn opt_str(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

fn opt_int(table: &toml::Table, key: &str, location: &str) -> Result<Option<i64>, FlyError> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_integer()
            .map(Some)
            .ok_or_else(|| FlyError::ConfigInvalid {
                location: format!("{location}.{key}"),
                detail: "must be an integer".into(),
            }),
    }
}

fn opt_string_array(
    table: &toml::Table,
    key: &str,
    location: &str,
) -> Result<Option<Vec<String>>, FlyError> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let array = value.as_array().ok_or_else(|| FlyError::ConfigInvalid {
        location: format!("{location}.{key}"),
        detail: "must be an array of strings".into(),
    })?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        let s = item.as_str().ok_or_else(|| FlyError::ConfigInvalid {
            location: format!("{location}.{key}"),
            detail: "every element must be a string".into(),
        })?;
        out.push(s.to_owned());
    }
    Ok(Some(out))
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
[stack.fly]
region = "iad"
[services.web]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/" }
[services.web.fly]
image = "hashicorp/http-echo"
internal_port = 5678
cmd = ["-text=stackless-smoke-ok", "-listen=:5678"]
"#;

    #[test]
    fn parses_image_block_with_defaults_and_overrides() {
        let def = parse(BASE);
        let svc = service_fly(&def, "web").unwrap();
        assert_eq!(svc.image(), Some("hashicorp/http-echo"));
        assert_eq!(svc.internal_port, 5678);
        assert_eq!(
            svc.cmd.as_deref(),
            Some(
                &[
                    "-text=stackless-smoke-ok".to_owned(),
                    "-listen=:5678".to_owned()
                ][..]
            )
        );
        assert_eq!(svc.guest, FlyGuest::default());
        assert_eq!(stack_region(&def), "iad");
        assert!(matches!(svc.mode, FlyDeployMode::Image { .. }));
    }

    #[test]
    fn source_build_defaults_dockerfile_when_image_absent() {
        let def = parse(
            r#"
[stack]
name = "atto"
[services.web]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/" }
[services.web.fly]
internal_port = 8080
"#,
        );
        let svc = service_fly(&def, "web").unwrap();
        assert_eq!(
            svc.mode,
            FlyDeployMode::Build {
                dockerfile: "Dockerfile".into()
            }
        );
        assert_eq!(svc.dockerfile(), Some("Dockerfile"));
        assert_eq!(svc.image(), None);
    }

    #[test]
    fn source_build_honors_explicit_dockerfile() {
        let def = parse(
            r#"
[stack]
name = "atto"
[services.web]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/" }
[services.web.fly]
dockerfile = "fixtures/smoke/fly-site/Dockerfile"
"#,
        );
        let svc = service_fly(&def, "web").unwrap();
        assert_eq!(svc.dockerfile(), Some("fixtures/smoke/fly-site/Dockerfile"));
    }

    #[test]
    fn image_and_dockerfile_together_rejected() {
        let toml = BASE.replace(
            "image = \"hashicorp/http-echo\"",
            "image = \"hashicorp/http-echo\"\ndockerfile = \"Dockerfile\"",
        );
        let err = service_fly(&parse(&toml), "web").unwrap_err();
        assert!(matches!(err, FlyError::ConfigInvalid { .. }));
        assert!(err.to_string().contains("either `image`"));
    }

    #[test]
    fn cmd_rejected_on_source_build() {
        let def = parse(
            r#"
[stack]
name = "atto"
[services.web]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/" }
[services.web.fly]
dockerfile = "Dockerfile"
cmd = ["server"]
"#,
        );
        let err = service_fly(&def, "web").unwrap_err();
        assert!(matches!(err, FlyError::ConfigInvalid { .. }));
        assert!(err.to_string().contains("`cmd`"));
    }

    #[test]
    fn region_defaults_when_absent() {
        let def = parse(
            r#"
[stack]
name = "atto"
[services.web]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/" }
[services.web.fly]
image = "nginx"
"#,
        );
        assert_eq!(stack_region(&def), "iad");
        let svc = service_fly(&def, "web").unwrap();
        assert_eq!(svc.internal_port, 8080);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let toml = BASE.replace(
            "image = \"hashicorp/http-echo\"",
            "bogus = 1\nimage = \"x\"",
        );
        let err = service_fly(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            crate::codes::FLY_CONFIG_INVALID
        );
    }

    #[test]
    fn missing_fly_block_is_rejected() {
        let toml = r#"
[stack]
name = "atto"
[services.web]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/" }
"#;
        let err = service_fly(&parse(toml), "web").unwrap_err();
        assert!(matches!(err, FlyError::ConfigInvalid { .. }));
    }

    #[test]
    fn app_name_pattern_matches_catalog() {
        assert!(is_valid_app_name("atto-demo-web"));
        assert!(is_valid_app_name("smoke-fly-1718-web"));
        assert!(!is_valid_app_name("ab")); // too short
        assert!(!is_valid_app_name("1abc")); // must start with a letter
        assert!(!is_valid_app_name("Abc")); // no uppercase
        assert!(!is_valid_app_name("a_b")); // underscore not allowed
        assert!(!is_valid_app_name(&"a".repeat(64))); // too long
    }

    #[test]
    fn typed_config_carries_its_catalog_reference() {
        assert_eq!(FlyAppConfig::REFERENCE, "flyio/app");
    }

    /// Catalog gap check: the `flyio/app` config must validate against the live
    /// `configuration_schema` in the committed catalog fixture. Fails loudly if
    /// Stripe drifts the `app_name` field.
    #[test]
    fn fly_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &FlyAppConfig {
                app_name: "atto-demo-web".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "fly catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
