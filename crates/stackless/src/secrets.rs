//! v0 secrets resolution (§0/§1): Stripe Projects vault pull is the base
//! when `[stack.projects.stripe].project` is recorded; a gitignored env file
//! next to stackless.toml overlays it — the override wins. Local-only stacks
//! without a Stripe anchor run env-file-only. A `required` key resolving from
//! neither fails before anything provisions, naming the sources consulted.

use std::collections::BTreeMap;
use std::path::Path;

use stackless_core::def::StackDef;
use stackless_stripe_projects::{merge_env_lines, vault_env_from_dir};

use crate::error::Error;

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

pub fn remember(
    store: &stackless_core::state::Store,
    owner_id: &str,
    values: &BTreeMap<String, String>,
) -> Result<(), Error> {
    store.remember_secrets(owner_id, values.values().map(String::as_str))?;
    let privileged = stackless_core::security::controller_values(values);
    store.remember_secrets(owner_id, privileged.iter().map(String::as_str))?;
    Ok(())
}

pub fn resolve_scoped(
    def: &StackDef,
    def_dir: &Path,
    runtime_dir: &Path,
    instance: &str,
    use_vault: bool,
) -> Result<BTreeMap<String, String>, Error> {
    let mut sources = Vec::new();
    let mut resolved = if use_vault {
        let vault = vault_env_from_dir(runtime_dir, (!instance.is_empty()).then_some(instance));
        if !vault.is_empty() {
            sources.push("Stripe Projects vault in the private instance runtime".into());
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

    let application = stackless_core::security::application_secrets(&resolved);
    if def.secrets.required.iter().any(|key| {
        stackless_core::security::controller_credential(key)
            || (resolved.contains_key(key) && !application.contains_key(key))
    }) {
        return Err(Error::BadArgument {
            argument: "secrets".into(),
            detail: "infrastructure credentials cannot be requested by application workloads"
                .into(),
        });
    }
    let missing: Vec<String> = def
        .secrets
        .required
        .iter()
        .filter(|key| !resolved.contains_key(*key))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(Error::SecretsUnresolved { missing, sources });
    }
    Ok(resolved)
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
        let resolved = resolve_scoped(
            &def,
            dir.path(),
            dir.path(),
            "",
            def.stack.projects.stripe.is_some(),
        )
        .unwrap();
        assert_eq!(
            resolved.get("API_TOKEN").map(String::as_str),
            Some("overlay-value")
        );
    }

    #[test]
    fn overlay_strips_single_quotes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ENV_FILE), "API_TOKEN='quoted'\n").unwrap();
        let loaded = load(dir.path());
        assert_eq!(loaded.get("API_TOKEN").map(String::as_str), Some("quoted"));
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
        let resolved = resolve_scoped(
            &def,
            dir.path(),
            dir.path(),
            "",
            def.stack.projects.stripe.is_some(),
        )
        .unwrap();
        assert_eq!(
            resolved.get("API_TOKEN").map(String::as_str),
            Some("file-only")
        );
    }
}
