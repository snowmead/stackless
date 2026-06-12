//! Parsing the Vercel-specific blocks of the definition.

use stackless_core::def::StackDef;

use crate::SUBSTRATE_NAME;
use crate::error::VercelError;

/// Stack-level `[stack.vercel]` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackVercel {
    pub plan: VercelPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VercelPlan {
    Hobby,
    Pro,
}

impl VercelPlan {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hobby => "hobby",
            Self::Pro => "pro",
        }
    }
}

/// A service's `[services.X.vercel]` block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceVercel {
    pub framework: Option<String>,
    pub build: Option<String>,
    pub install: Option<String>,
    pub root: Option<String>,
    pub output: Option<String>,
}

impl StackVercel {
    pub fn parse(def: &StackDef) -> Self {
        let plan = def
            .stack
            .substrates
            .get(SUBSTRATE_NAME)
            .and_then(|value| value.as_table())
            .and_then(|table| table.get("plan"))
            .and_then(|value| value.as_str())
            .and_then(VercelPlan::parse)
            .unwrap_or(VercelPlan::Hobby);
        Self { plan }
    }

    pub fn validate(def: &StackDef) -> Result<Self, VercelError> {
        let location = "stack.vercel";
        let Some(block) = def
            .stack
            .substrates
            .get(SUBSTRATE_NAME)
            .and_then(|value| value.as_table())
        else {
            return Ok(Self {
                plan: VercelPlan::Hobby,
            });
        };
        for key in block.keys() {
            if key != "plan" {
                return Err(VercelError::ConfigInvalid {
                    location: location.into(),
                    detail: format!("unknown key {key:?} (known: plan)"),
                });
            }
        }
        let plan = match block.get("plan") {
            None => VercelPlan::Hobby,
            Some(value) => {
                let Some(raw) = value.as_str() else {
                    return Err(VercelError::ConfigInvalid {
                        location: format!("{location}.plan"),
                        detail: "must be a string".into(),
                    });
                };
                VercelPlan::parse(raw).ok_or_else(|| VercelError::ConfigInvalid {
                    location: format!("{location}.plan"),
                    detail: format!("unknown plan {raw:?} (known: hobby, pro)"),
                })?
            }
        };
        Ok(Self { plan })
    }
}

impl VercelPlan {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "hobby" => Some(Self::Hobby),
            "pro" => Some(Self::Pro),
            _ => None,
        }
    }
}

impl ServiceVercel {
    pub fn parse(def: &StackDef, service: &str) -> Result<Self, VercelError> {
        let location = format!("services.{service}.vercel");
        let block = def
            .services
            .get(service)
            .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
            .and_then(|value| value.as_table())
            .ok_or_else(|| VercelError::ConfigInvalid {
                location: location.clone(),
                detail: "missing [services.X.vercel] block".into(),
            })?;

        for key in block.keys() {
            if !matches!(
                key.as_str(),
                "framework" | "build" | "install" | "root" | "output" | "env"
            ) {
                return Err(VercelError::ConfigInvalid {
                    location: location.clone(),
                    detail: format!(
                        "unknown key {key:?} (known: framework, build, install, root, output, env)"
                    ),
                });
            }
        }

        Ok(Self {
            framework: optional_str(block, "framework", &location)?,
            build: optional_str(block, "build", &location)?,
            install: optional_str(block, "install", &location)?,
            root: optional_str(block, "root", &location)?,
            output: optional_str(block, "output", &location)?,
        })
    }
}

fn optional_str(
    table: &toml::Table,
    key: &str,
    location: &str,
) -> Result<Option<String>, VercelError> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => {
            let Some(text) = value.as_str() else {
                return Err(VercelError::ConfigInvalid {
                    location: format!("{location}.{key}"),
                    detail: "must be a string".into(),
                });
            };
            if text.trim().is_empty() {
                return Err(VercelError::ConfigInvalid {
                    location: format!("{location}.{key}"),
                    detail: "must not be empty".into(),
                });
            }
            Ok(Some(text.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use stackless_core::def::StackDef;
    use stackless_core::fault::Fault;

    use super::*;

    fn parse(toml: &str) -> StackDef {
        StackDef::parse(toml).expect("valid base toml")
    }

    #[test]
    fn missing_vercel_block_is_rejected() {
        let def = parse(
            r#"
[stack]
name = "demo"
[services.web]
source = { repo = "https://github.com/acme/web", ref = "main" }
health = { path = "/" }
"#,
        );
        let err = ServiceVercel::parse(&def, "web").unwrap_err();
        assert_eq!(
            err.code(),
            stackless_core::fault::codes::VERCEL_CONFIG_INVALID
        );
    }

    #[test]
    fn parses_optional_service_fields() {
        let def = parse(
            r#"
[stack]
name = "demo"
[services.web]
source = { repo = "https://github.com/acme/web", ref = "main" }
health = { path = "/" }
[services.web.vercel]
framework = "vite"
build = "npm run build"
"#,
        );
        let cfg = ServiceVercel::parse(&def, "web").unwrap();
        assert_eq!(cfg.framework.as_deref(), Some("vite"));
        assert_eq!(cfg.build.as_deref(), Some("npm run build"));
    }

    #[test]
    fn stack_plan_defaults_to_hobby() {
        let def = parse("[stack]\nname = \"demo\"\n");
        assert_eq!(StackVercel::parse(&def).plan, VercelPlan::Hobby);
    }
}
