//! Parsing the render-specific blocks of the definition (§1 schema).
//!
//! `validate_definition` checks these shapes strictly — unknown keys are
//! a fault (agent-trap protection, mirroring stackless-local). The same
//! parsers feed the Substrate impl so config is read in exactly one place.

use serde::Serialize;
use stackless_core::def::StackDef;

use crate::RenderSubstrate;
use crate::SUBSTRATE_NAME;
use crate::error::RenderError;
use stackless_stripe_projects::CatalogService;
use stackless_stripe_projects::stripe::CommandRunner;

/// A service's `[services.X.render]` block: either a runtime web service
/// or a static site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceRender {
    Web {
        runtime: String,
        build: String,
        start: String,
    },
    Static {
        build: String,
        publish: String,
        spa_rewrite: bool,
    },
}

impl ServiceRender {
    pub fn is_static(&self) -> bool {
        matches!(self, Self::Static { .. })
    }
}

/// The typed `render/postgres` `--config`. `name`/`region`/`version` are schema
/// properties; `instance_type` is a pricing-tier selector (`basic-256mb`, …).
/// Field names ARE the catalog contract — the gap test pins them.
#[derive(Debug, Serialize)]
pub struct RenderPostgresConfig {
    pub name: String,
    pub region: String,
    pub version: String,
    pub instance_type: String,
}

impl CatalogService for RenderPostgresConfig {
    const REFERENCE: &'static str = "render/postgres";
}

/// The typed `render/web-service` `--config`.
#[derive(Debug, Serialize)]
pub struct RenderWebServiceConfig {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub runtime: String,
    pub build_command: String,
    pub start_command: String,
    pub health_check_path: String,
    pub region: String,
    pub auto_deploy: String,
}

impl CatalogService for RenderWebServiceConfig {
    const REFERENCE: &'static str = "render/web-service";
}

/// The typed `render/static-site` `--config`.
#[derive(Debug, Serialize)]
pub struct RenderStaticSiteConfig {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub build_command: String,
    pub publish_path: String,
}

impl CatalogService for RenderStaticSiteConfig {
    const REFERENCE: &'static str = "render/static-site";
}

impl<R: CommandRunner> RenderSubstrate<R> {
    /// Read and shape-check `[services.<service>.render]`.
    pub fn service_render(def: &StackDef, service: &str) -> Result<ServiceRender, RenderError> {
        let location = format!("services.{service}.render");
        let block = def
            .services
            .get(service)
            .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
            .and_then(|value| value.as_table())
            .ok_or_else(|| RenderError::ConfigInvalid {
                location: location.clone(),
                detail: "missing [services.X.render] block".into(),
            })?;

        // `static` and the web triple are mutually exclusive.
        if let Some(static_value) = block.get("static") {
            for key in block.keys() {
                if !matches!(key.as_str(), "static" | "env") {
                    return Err(RenderError::ConfigInvalid {
                        location: location.clone(),
                        detail: format!(
                            "unknown key {key:?} alongside `static` (known: static, env)"
                        ),
                    });
                }
            }
            let table = static_value
                .as_table()
                .ok_or_else(|| RenderError::ConfigInvalid {
                    location: format!("{location}.static"),
                    detail: "must be a table { build, publish, spa_rewrite? }".into(),
                })?;
            for key in table.keys() {
                if !matches!(key.as_str(), "build" | "publish" | "spa_rewrite") {
                    return Err(RenderError::ConfigInvalid {
                        location: format!("{location}.static"),
                        detail: format!("unknown key {key:?} (known: build, publish, spa_rewrite)"),
                    });
                }
            }
            let build = required_str(table, "build", &format!("{location}.static"))?;
            let publish = required_str(table, "publish", &format!("{location}.static"))?;
            let spa_rewrite = match table.get("spa_rewrite") {
                None => false,
                Some(v) => v.as_bool().ok_or_else(|| RenderError::ConfigInvalid {
                    location: format!("{location}.static.spa_rewrite"),
                    detail: "must be a boolean".into(),
                })?,
            };
            return Ok(ServiceRender::Static {
                build,
                publish,
                spa_rewrite,
            });
        }

        // Web service: runtime + build + start (+ optional env).
        for key in block.keys() {
            if !matches!(key.as_str(), "runtime" | "build" | "start" | "env") {
                return Err(RenderError::ConfigInvalid {
                    location: location.clone(),
                    detail: format!(
                        "unknown key {key:?} (known: runtime, build, start, env — or use `static`)"
                    ),
                });
            }
        }
        Ok(ServiceRender::Web {
            runtime: required_str(block, "runtime", &location)?,
            build: required_str(block, "build", &location)?,
            start: required_str(block, "start", &location)?,
        })
    }

    /// A datastore's `[datastores.X.render]` plan.
    pub fn datastore_plan(def: &StackDef, datastore: &str) -> Result<String, RenderError> {
        let location = format!("datastores.{datastore}.render");
        let block = def
            .datastores
            .get(datastore)
            .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
            .and_then(|value| value.as_table())
            .ok_or_else(|| RenderError::ConfigInvalid {
                location: location.clone(),
                detail: "missing [datastores.X.render] block".into(),
            })?;
        for key in block.keys() {
            if key.as_str() != "plan" {
                return Err(RenderError::ConfigInvalid {
                    location: location.clone(),
                    detail: format!("unknown key {key:?} (known: plan)"),
                });
            }
        }
        required_str(block, "plan", &location)
    }

    /// The recorded Stripe Projects anchor from `[stack.projects.stripe].project`.
    pub fn stack_project(def: &StackDef) -> Option<String> {
        stackless_stripe_projects::recorded_project_id(def)
    }

    /// The recorded `[stack.render].region`, defaulting to oregon (§1).
    pub fn stack_region(def: &StackDef) -> String {
        def.stack
            .substrates
            .get(SUBSTRATE_NAME)
            .and_then(|value| value.as_table())
            .and_then(|table| table.get("region"))
            .and_then(|value| value.as_str())
            .unwrap_or("oregon")
            .to_owned()
    }
}

fn required_str(table: &toml::Table, key: &str, location: &str) -> Result<String, RenderError> {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| RenderError::ConfigInvalid {
            location: location.to_owned(),
            detail: format!("missing or empty `{key}`"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_stripe_projects::TokioRunner;

    fn parse(toml: &str) -> StackDef {
        StackDef::parse(toml).expect("valid base toml")
    }

    #[test]
    fn stack_project_reads_neutral_anchor() {
        let def = parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_neutral"
"#,
        );
        assert_eq!(
            RenderSubstrate::<TokioRunner>::stack_project(&def).as_deref(),
            Some("project_neutral")
        );
    }

    const BASE: &str = r#"
[stack]
name = "atto"
[datastores.db]
engine = "postgres"
version = "17"
[datastores.db.render]
plan = "basic-256mb"
[services.api]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/health" }
[services.api.render]
runtime = "rust"
build = "cargo build --release"
start = "./bin"
[services.web]
source = { repo = "r", ref = "main" }
env = {}
health = { path = "/" }
[services.web.render]
static = { build = "bun run build", publish = "./dist", spa_rewrite = true }
"#;

    #[test]
    fn parses_web_and_static_blocks() {
        let def = parse(BASE);
        assert!(matches!(
            RenderSubstrate::<TokioRunner>::service_render(&def, "api").unwrap(),
            ServiceRender::Web { .. }
        ));
        let web = RenderSubstrate::<TokioRunner>::service_render(&def, "web").unwrap();
        assert!(web.is_static());
        assert_eq!(
            RenderSubstrate::<TokioRunner>::datastore_plan(&def, "db").unwrap(),
            "basic-256mb"
        );
        assert_eq!(RenderSubstrate::<TokioRunner>::stack_region(&def), "oregon");
    }

    #[test]
    fn typed_configs_carry_their_catalog_references() {
        assert_eq!(RenderWebServiceConfig::REFERENCE, "render/web-service");
        assert_eq!(RenderStaticSiteConfig::REFERENCE, "render/static-site");
        assert_eq!(RenderPostgresConfig::REFERENCE, "render/postgres");
    }

    /// Catalog gap check: each Render config must validate against the live
    /// `configuration_schema` + pricing tiers in the committed catalog fixture.
    /// Fails loudly if Stripe drifts a field/region/runtime/tier.
    #[test]
    fn render_configs_match_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let mut failures = Vec::new();
        failures.extend(stackless_stripe_projects::verify_service(
            &catalog,
            &RenderPostgresConfig {
                name: "atto-demo-db".into(),
                region: "oregon".into(),
                version: "17".into(),
                instance_type: "basic-256mb".into(),
            },
        ));
        failures.extend(stackless_stripe_projects::verify_service(
            &catalog,
            &RenderWebServiceConfig {
                name: "atto-demo-api".into(),
                repo: "https://github.com/haaku-co/atto-server".into(),
                branch: "main".into(),
                runtime: "rust".into(),
                build_command: "cargo build --release".into(),
                start_command: "./target/release/atto-server".into(),
                health_check_path: "/health".into(),
                region: "oregon".into(),
                auto_deploy: "no".into(),
            },
        ));
        failures.extend(stackless_stripe_projects::verify_service(
            &catalog,
            &RenderStaticSiteConfig {
                name: "atto-demo-web".into(),
                repo: "https://github.com/haaku-co/atto-web".into(),
                branch: "main".into(),
                build_command: "bun run build".into(),
                publish_path: "./dist".into(),
            },
        ));
        assert!(
            failures.is_empty(),
            "render catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn unknown_key_in_web_block_is_rejected() {
        let toml = BASE.replace(
            "[services.api.render]\nruntime = \"rust\"",
            "[services.api.render]\nbogus = \"x\"\nruntime = \"rust\"",
        );
        let err = RenderSubstrate::<TokioRunner>::service_render(&parse(&toml), "api").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            stackless_core::fault::codes::RENDER_CONFIG_INVALID
        );
    }

    #[test]
    fn unknown_key_in_static_block_is_rejected() {
        let toml = BASE.replace(
            "static = { build = \"bun run build\", publish = \"./dist\", spa_rewrite = true }",
            "static = { build = \"b\", publish = \"./dist\", bogus = 1 }",
        );
        let err = RenderSubstrate::<TokioRunner>::service_render(&parse(&toml), "web").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            stackless_core::fault::codes::RENDER_CONFIG_INVALID
        );
    }

    #[test]
    fn missing_render_block_is_rejected() {
        let toml = BASE.replace(
            "[services.api.render]\nruntime = \"rust\"\nbuild = \"cargo build --release\"\nstart = \"./bin\"",
            "",
        );
        let err = RenderSubstrate::<TokioRunner>::service_render(&parse(&toml), "api").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            stackless_core::fault::codes::RENDER_CONFIG_INVALID
        );
    }

    #[test]
    fn datastore_missing_plan_is_rejected() {
        let toml = BASE.replace("plan = \"basic-256mb\"", "");
        let err = RenderSubstrate::<TokioRunner>::datastore_plan(&parse(&toml), "db").unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&err),
            stackless_core::fault::codes::RENDER_CONFIG_INVALID
        );
    }
}
