//! v0 secrets resolution (§0/§1): Stripe Projects vault pull is the base
//! when `[stack.projects.stripe].project` is recorded; a gitignored env file
//! next to stackless.toml overlays it — the override wins. Local-only stacks
//! without a Stripe anchor run env-file-only. A `required` key resolving from
//! neither fails before anything provisions, naming the sources consulted.

use std::collections::BTreeMap;
use std::path::Path;

use stackless_core::def::StackDef;
use stackless_stripe_projects::{recorded_project_id, vault_env_from_dir};

use crate::error::CliError;

pub const ENV_FILE: &str = ".stackless.env";

/// Load the `.stackless.env` overlay into a map. Best-effort: an absent file
/// yields an empty map. Does NOT enforce `[secrets].required` — use for
/// read-only paths (e.g. `logs`) that only need whatever keys happen to be set.
pub fn load(def_dir: &Path) -> BTreeMap<String, String> {
    let mut resolved = BTreeMap::new();
    let env_path = def_dir.join(ENV_FILE);
    if let Ok(content) = std::fs::read_to_string(&env_path) {
        merge_env_lines(&mut resolved, &content);
    }
    resolved
}

pub fn resolve(
    def: &StackDef,
    def_dir: &Path,
    instance: Option<&str>,
) -> Result<BTreeMap<String, String>, CliError> {
    let mut sources = Vec::new();
    let mut resolved = if recorded_project_id(def).is_some() {
        let vault = vault_env_from_dir(def_dir, instance);
        if !vault.is_empty() {
            sources.push("Stripe Projects vault (.env / .env.<instance>)".into());
        }
        vault
    } else {
        BTreeMap::new()
    };

    let env_path = def_dir.join(ENV_FILE);
    let overlay = load(def_dir);
    if env_path.exists() {
        sources.push(env_path.display().to_string());
    } else {
        sources.push(format!("{} (absent)", env_path.display()));
    }
    for (key, value) in overlay {
        resolved.insert(key, value);
    }

    let missing: Vec<String> = def
        .secrets
        .required
        .iter()
        .filter(|key| !resolved.contains_key(*key))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(CliError::SecretsUnresolved { missing, sources });
    }
    Ok(resolved)
}

fn merge_env_lines(out: &mut BTreeMap<String, String>, content: &str) {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            out.insert(
                key.trim().to_owned(),
                value.trim().trim_matches('"').to_owned(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_wins_over_vault_base() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "API_TOKEN=vault-value\n").unwrap();
        std::fs::write(dir.path().join(ENV_FILE), "API_TOKEN=overlay-value\n").unwrap();
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[stack.projects.stripe]
project = "project_test"
[secrets]
required = ["API_TOKEN"]
"#,
        )
        .unwrap();
        let resolved = resolve(&def, dir.path(), None).unwrap();
        assert_eq!(
            resolved.get("API_TOKEN").map(String::as_str),
            Some("overlay-value")
        );
    }

    #[test]
    fn local_stack_uses_env_file_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ENV_FILE), "API_TOKEN=file-only\n").unwrap();
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[secrets]
required = ["API_TOKEN"]
"#,
        )
        .unwrap();
        let resolved = resolve(&def, dir.path(), None).unwrap();
        assert_eq!(
            resolved.get("API_TOKEN").map(String::as_str),
            Some("file-only")
        );
    }
}
