//! `stackless adopt` — inspect the repo and write or merge a draft
//! `stackless.toml`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::authoring::{
    GitSource, append_service_block, default_output_path, default_source, definition_dir,
    ensure_trailing_newline, resolve_stack_name, service_block_present,
};
use crate::error::CliError;
use crate::output::Output;

pub struct AdoptArgs {
    pub name: Option<String>,
    pub file: Option<PathBuf>,
    pub force: bool,
    pub merge: bool,
}

#[derive(Debug, Clone)]
struct DetectedService {
    name: String,
    block: String,
    root_origin: bool,
}

pub fn adopt(args: AdoptArgs, output: &Output) -> Result<(), CliError> {
    let file = args.file.unwrap_or_else(default_output_path);
    let dir = definition_dir(&file);
    let stack = resolve_stack_name(args.name.as_deref(), &dir)?;
    let source = default_source(&dir);
    let detected = detect_services(&dir, &stack, &source)?;
    let (text, merged) = if file.exists() {
        if args.force {
            (compose_definition(&stack, &detected), false)
        } else if args.merge {
            let existing = std::fs::read_to_string(&file).map_err(|source| CliError::FileRead {
                path: file.display().to_string(),
                source,
            })?;
            let mut text = existing;
            let mut merged = false;
            for service in &detected {
                if service_block_present(&text, &service.name) {
                    continue;
                }
                text = append_service_block(&text, &service.block);
                merged = true;
            }
            if !text.contains("[stack]") {
                let header = format!("[stack]\nname = \"{stack}\"\n\n");
                text = format!("{header}{text}");
                merged = true;
            }
            (ensure_trailing_newline(&text), merged)
        } else {
            return Err(CliError::AdoptExists {
                path: file.display().to_string(),
            });
        }
    } else {
        (compose_definition(&stack, &detected), false)
    };
    stackless_core::def::StackDef::parse(&text)?;
    std::fs::write(&file, &text).map_err(|source| CliError::FileWrite {
        path: file.display().to_string(),
        source,
    })?;
    let services: Vec<&str> = detected.iter().map(|s| s.name.as_str()).collect();
    output.adopt_ok(
        file.display().to_string(),
        &services,
        merged,
        "stackless check stackless.toml --on local",
    );
    Ok(())
}

fn compose_definition(stack: &str, services: &[DetectedService]) -> String {
    let mut out = format!("[stack]\nname = \"{stack}\"\n\n");
    let mut root_set = false;
    for service in services {
        let block = if service.root_origin && !root_set {
            root_set = true;
            service
                .block
                .replace("root_origin = false", "root_origin = true")
        } else {
            service.block.clone()
        };
        out.push_str(&block);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn detect_services(
    dir: &Path,
    stack: &str,
    source: &GitSource,
) -> Result<Vec<DetectedService>, CliError> {
    let mut services = Vec::new();
    let cargo = dir.join("Cargo.toml");
    let package = dir.join("package.json");
    let index = dir.join("index.html");

    if package.is_file()
        && let Some(service) = detect_node_service(dir, stack, source)?
    {
        services.push(service);
    }
    if cargo.is_file() && !services.iter().any(|s| s.name == "api") {
        services.push(rust_api_service(stack, source));
    }
    if services.is_empty() && index.is_file() {
        services.push(static_web_service(stack, source, true));
    }
    if services.is_empty() {
        services.push(static_web_service(stack, source, true));
    }
    Ok(services)
}

fn detect_node_service(
    dir: &Path,
    stack: &str,
    source: &GitSource,
) -> Result<Option<DetectedService>, CliError> {
    let text =
        std::fs::read_to_string(dir.join("package.json")).map_err(|source| CliError::FileRead {
            path: dir.join("package.json").display().to_string(),
            source,
        })?;
    let value: Value = serde_json::from_str(&text).map_err(|err| CliError::AdoptInspect {
        path: "package.json".into(),
        detail: err.to_string(),
    })?;
    let uses_vite = value
        .pointer("/devDependencies/vite")
        .or_else(|| value.pointer("/dependencies/vite"))
        .is_some();
    if uses_vite {
        return Ok(Some(DetectedService {
            name: "web".into(),
            block: format!(
                r#"[services.web]
source = {{ repo = "{repo}", ref = "{git_ref}" }}
root_origin = true
health = {{ path = "/", contains = 'id="root"' }}

  [services.web.local]
  run = "bunx vite --host 127.0.0.1 --port $PORT --strictPort"
"#,
                repo = source.repo,
                git_ref = source.git_ref,
            ),
            root_origin: true,
        }));
    }
    if dir.join("index.html").is_file() {
        return Ok(Some(static_web_service(stack, source, true)));
    }
    Ok(None)
}

fn rust_api_service(stack: &str, source: &GitSource) -> DetectedService {
    let _ = stack;
    DetectedService {
        name: "api".into(),
        block: format!(
            r#"[services.api]
source = {{ repo = "{repo}", ref = "{git_ref}" }}
health = {{ path = "/health", contains = "ok" }}

  [services.api.local]
  run = "cargo run"
"#,
            repo = source.repo,
            git_ref = source.git_ref,
        ),
        root_origin: false,
    }
}

fn static_web_service(stack: &str, source: &GitSource, root_origin: bool) -> DetectedService {
    let _ = stack;
    DetectedService {
        name: "web".into(),
        block: format!(
            r#"[services.web]
source = {{ repo = "{repo}", ref = "{git_ref}" }}
root_origin = {root_origin}
health = {{ path = "/", contains = "html" }}

  [services.web.local]
  run = "python3 -m http.server $PORT --bind 127.0.0.1"
"#,
            repo = source.repo,
            git_ref = source.git_ref,
            root_origin = root_origin,
        ),
        root_origin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_api_from_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"api\"\n").unwrap();
        let source = GitSource {
            repo: "file:///tmp/repo".into(),
            git_ref: "main".into(),
        };
        let services = detect_services(dir.path(), "demo", &source).unwrap();
        assert!(services.iter().any(|s| s.name == "api"));
    }

    #[test]
    fn detects_static_from_index_html() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        let source = GitSource {
            repo: "file:///tmp/repo".into(),
            git_ref: "main".into(),
        };
        let services = detect_services(dir.path(), "demo", &source).unwrap();
        assert_eq!(services[0].name, "web");
    }
}
