//! `stackless adopt` — inspect the repo and write or merge a draft
//! `stackless.toml`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::authoring::{
    GitSource, append_service_block, default_output_path, default_source, definition_dir,
    ensure_trailing_newline, resolve_stack_name, service_block_present, stack_section_present,
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
            (
                compose_definition(&stack, &detected, &adopt_notes(&dir)),
                false,
            )
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
            if !stack_section_present(&text) {
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
        (
            compose_definition(&stack, &detected, &adopt_notes(&dir)),
            false,
        )
    };
    stackless_core::def::StackDef::parse(&text)?;
    std::fs::write(&file, &text).map_err(|source| CliError::FileWrite {
        path: file.display().to_string(),
        source,
    })?;
    let services: Vec<&str> = detected.iter().map(|s| s.name.as_str()).collect();
    let next = format!("stackless check {} --on local", file.display());
    output.adopt_ok(file.display().to_string(), &services, merged, &next);
    Ok(())
}

fn compose_definition(stack: &str, services: &[DetectedService], notes: &[String]) -> String {
    let mut out = String::new();
    for note in notes {
        out.push_str(note);
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!("[stack]\nname = \"{stack}\"\n\n"));
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

fn adopt_notes(dir: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    if dir.join("docker-compose.yml").is_file() || dir.join("compose.yaml").is_file() {
        notes.push(
            "# adopt: found docker-compose — stackless does not translate compose files; \
             declare each service explicitly."
                .into(),
        );
    }
    if dir.join("Dockerfile").is_file() {
        notes.push(
            "# adopt: found Dockerfile — local substrate runs host processes, not container \
             images (use a cloud substrate or hand-write run commands)."
                .into(),
        );
    }
    notes
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
        && let Some(service) = detect_node_service(dir, stack, source)
    {
        services.push(service);
    }
    if (dir.join("pyproject.toml").is_file() || dir.join("requirements.txt").is_file())
        && let Some(service) = detect_python_service(dir, stack, source)
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

fn detect_node_service(dir: &Path, stack: &str, source: &GitSource) -> Option<DetectedService> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let uses_vite = value
        .pointer("/devDependencies/vite")
        .or_else(|| value.pointer("/dependencies/vite"))
        .is_some();
    if uses_vite {
        return Some(DetectedService {
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
        });
    }
    if let Some(scripts) = value.get("scripts").and_then(|v| v.as_object()) {
        let run = if scripts.contains_key("dev") {
            "npm run dev -- --port $PORT"
        } else if scripts.contains_key("start") {
            "npm start"
        } else {
            return None;
        };
        return Some(DetectedService {
            name: "web".into(),
            block: format!(
                r#"[services.web]
source = {{ repo = "{repo}", ref = "{git_ref}" }}
root_origin = true
health = {{ path = "/", status = 200 }}

  [services.web.local]
  run = "{run}"
"#,
                repo = source.repo,
                git_ref = source.git_ref,
                run = run,
            ),
            root_origin: true,
        });
    }
    if dir.join("index.html").is_file() {
        return Some(static_web_service(stack, source, true));
    }
    None
}

fn detect_python_service(dir: &Path, stack: &str, source: &GitSource) -> Option<DetectedService> {
    let _ = stack;
    let run = if dir.join("manage.py").is_file() {
        "python3 manage.py runserver 127.0.0.1:$PORT"
    } else if dir.join("app.py").is_file() {
        "python3 app.py"
    } else if dir.join("main.py").is_file() {
        "python3 main.py"
    } else {
        return None;
    };
    Some(DetectedService {
        name: "api".into(),
        block: format!(
            r#"[services.api]
source = {{ repo = "{repo}", ref = "{git_ref}" }}
health = {{ path = "/", status = 200 }}

  [services.api.local]
  run = "{run}"
"#,
            repo = source.repo,
            git_ref = source.git_ref,
            run = run,
        ),
        root_origin: false,
    })
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
    use crate::authoring::{append_service_block, service_block_present, stack_section_present};

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

    #[test]
    fn skips_invalid_package_json_and_detects_cargo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{not json").unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"api\"\n").unwrap();
        let source = GitSource {
            repo: "file:///tmp/repo".into(),
            git_ref: "main".into(),
        };
        let services = detect_services(dir.path(), "demo", &source).unwrap();
        assert!(services.iter().any(|s| s.name == "api"));
    }

    #[test]
    fn merge_prepends_stack_header_when_only_nested_stack_tables() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        let file = dir.path().join("stackless.toml");
        std::fs::write(&file, "[stack.verify]\nrun = \"true\"\n").unwrap();
        let source = GitSource {
            repo: "file:///tmp/repo".into(),
            git_ref: "main".into(),
        };
        let detected = detect_services(dir.path(), "demo", &source).unwrap();
        let mut text = std::fs::read_to_string(&file).unwrap();
        assert!(!stack_section_present(&text));
        if !stack_section_present(&text) {
            let header = "[stack]\nname = \"demo\"\n\n";
            text = format!("{header}{text}");
        }
        for service in &detected {
            if !service_block_present(&text, &service.name) {
                text = append_service_block(&text, &service.block);
            }
        }
        stackless_core::def::StackDef::parse(&text).unwrap();
    }
}
