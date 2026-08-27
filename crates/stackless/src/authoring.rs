//! Shared helpers for `init` and `adopt`: git probing, DNS-safe names, and
//! template fragments.

use std::path::{Path, PathBuf};

use stackless_core::types::DnsName;

use crate::error::Error;

/// The Stripe Projects plugin version stackless is tested against.
pub const STRIPE_PROJECTS_PINNED: &str = "0.36.0";

pub fn default_output_path() -> PathBuf {
    PathBuf::from("stackless.toml")
}

pub fn definition_dir(file: &Path) -> PathBuf {
    let parent = file.parent().filter(|p| !p.as_os_str().is_empty());
    parent
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn stack_name_from_dir(dir: &Path) -> String {
    let raw = dir.file_name().and_then(|n| n.to_str()).unwrap_or("stack");
    sanitize_dns_name(raw).unwrap_or_else(|| "stack".into())
}

pub fn sanitize_dns_name(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut prev_hyphen = false;
    for ch in raw.to_ascii_lowercase().chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '-';
        let mapped = if ok { ch } else { '-' };
        if mapped == '-' {
            if prev_hyphen {
                continue;
            }
            prev_hyphen = true;
        } else {
            prev_hyphen = false;
        }
        out.push(mapped);
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() || out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, 's');
        if out.len() > 1 && out.as_bytes()[1] == b'-' {
            out.remove(1);
        }
    }
    if out.len() > 63 {
        out.truncate(63);
        while out.ends_with('-') {
            out.pop();
        }
    }
    DnsName::try_new(&out).ok().map(DnsName::into_inner)
}

pub fn resolve_stack_name(name: Option<&str>, dir: &Path) -> Result<String, Error> {
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| stack_name_from_dir(dir));
    DnsName::try_new(&name)
        .map(DnsName::into_inner)
        .map_err(|err| Error::InitNameInvalid {
            name,
            detail: err.to_string(),
        })
}

pub struct GitSource {
    pub repo: String,
    pub git_ref: String,
}

pub fn detect_git_source(dir: &Path) -> Option<GitSource> {
    let repo = git_output(dir, &["remote", "get-url", "origin"])?;
    let git_ref = match git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some(r) if r != "HEAD" => r,
        _ => git_output(dir, &["rev-parse", "HEAD"]).unwrap_or_else(|| "main".into()),
    };
    Some(GitSource { repo, git_ref })
}

pub fn default_source(dir: &Path) -> GitSource {
    detect_git_source(dir).unwrap_or_else(|| {
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        GitSource {
            repo: format!("file://{}", canonical.display()),
            git_ref: "main".into(),
        }
    })
}

fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

pub fn static_web_template(stack: &str, source: &GitSource) -> String {
    format!(
        r#"[stack]
name = "{stack}"

[services.web]
source = {{ repo = "{repo}", ref = "{git_ref}" }}
root_origin = true
health = {{ path = "/", contains = "html" }}

  [services.web.local]
  run = "python3 -m http.server $PORT --bind 127.0.0.1"
"#,
        stack = stack,
        repo = source.repo,
        git_ref = source.git_ref,
    )
}

pub fn service_block_present(text: &str, service: &str) -> bool {
    text.contains(&format!("[services.{service}]"))
}

pub fn stack_section_present(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "[stack]")
}

pub fn append_service_block(existing: &str, block: &str) -> String {
    let mut out = existing.trim_end().to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(block.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub fn ensure_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_dns_name_maps_spaces_and_digits() {
        assert_eq!(sanitize_dns_name("My-App").as_deref(), Some("my-app"));
        assert_eq!(sanitize_dns_name("123demo").as_deref(), Some("s123demo"));
    }

    #[test]
    fn static_template_is_dns_safe_stack_name() {
        let source = GitSource {
            repo: "https://github.com/you/hello".into(),
            git_ref: "main".into(),
        };
        let text = static_web_template("hello", &source);
        let def = stackless_core::def::StackDef::parse(&text).unwrap();
        assert_eq!(def.stack.name.as_str(), "hello");
        assert!(stackless_core::types::dns_safe(def.stack.name.as_str()));
    }
}
