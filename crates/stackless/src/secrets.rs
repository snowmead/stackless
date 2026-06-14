//! v0 secrets resolution (§0/§1): the stack's vault pull is the base
//! (lands with the Stripe Projects driver, M8) and a gitignored env
//! file next to stackless.toml overlays it — the override wins (the
//! Clerk lesson's hand-managed keys). A `required` key resolving from
//! neither fails before anything provisions, naming the sources
//! consulted.

use std::collections::BTreeMap;
use std::path::Path;

use stackless_core::def::StackDef;

use crate::error::CliError;

pub const ENV_FILE: &str = ".stackless.env";

/// Load the `.stackless.env` overlay into a map. Best-effort: an absent file
/// yields an empty map. Does NOT enforce `[secrets].required` — use for
/// read-only paths (e.g. `logs`) that only need whatever keys happen to be set.
pub fn load(def_dir: &Path) -> BTreeMap<String, String> {
    let mut resolved = BTreeMap::new();
    let env_path = def_dir.join(ENV_FILE);
    if let Ok(content) = std::fs::read_to_string(&env_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                resolved.insert(
                    key.trim().to_owned(),
                    value.trim().trim_matches('"').to_owned(),
                );
            }
        }
    }
    resolved
}

pub fn resolve(def: &StackDef, def_dir: &Path) -> Result<BTreeMap<String, String>, CliError> {
    // Vault base: not configured until the render substrate records a
    // Stripe project (M8). Local-only stacks legally run env-file-only.
    let resolved = load(def_dir);
    let env_path = def_dir.join(ENV_FILE);
    let sources = vec![if env_path.exists() {
        env_path.display().to_string()
    } else {
        format!("{} (absent)", env_path.display())
    }];

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
