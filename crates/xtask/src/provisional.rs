//! Inventory + CI gate for unpinned credential envelopes.
//!
//! Scans **declared** modules under `crates/stackless-integrations/src/providers/`
//! (reachable from `providers/mod.rs` via `mod` / `pub mod`) for
//! `Provisional until` / `Best-guess` markers next to `OUTPUT_FIELDS`, maps each
//! hit to its module's `CatalogService::REFERENCE`, and compares against the
//! committed allowlist. Held/EXCL'd sources that omit the parent `pub mod` are
//! ignored.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

type Fail = Box<dyn Error>;

const MARKER_PROVISIONAL: &str = "Provisional until";
const MARKER_BEST_GUESS: &str = "Best-guess";

pub fn providers_dir(workspace: &Path) -> PathBuf {
    workspace.join("crates/stackless-integrations/src/providers")
}

pub fn allowlist_path(workspace: &Path) -> PathBuf {
    workspace.join("crates/stackless-integrations/provisional-allowlist.txt")
}

/// Sorted catalog refs that still carry a Provisional / Best-guess marker.
pub fn scan_provisional_refs(providers: &Path) -> Result<Vec<String>, Fail> {
    let mut refs = BTreeSet::new();
    for path in rust_files(providers)? {
        let text = fs::read_to_string(&path)?;
        if !text.contains(MARKER_PROVISIONAL) && !text.contains(MARKER_BEST_GUESS) {
            continue;
        }
        // Only count files that mark OUTPUT_FIELDS (not module-level prose).
        if !marks_output_fields(&text) {
            continue;
        }
        let reference = extract_reference(&text).ok_or_else(|| {
            format!(
                "{}: has {MARKER_PROVISIONAL}/{MARKER_BEST_GUESS} but no CatalogService::REFERENCE",
                path.display()
            )
        })?;
        refs.insert(reference);
    }
    Ok(refs.into_iter().collect())
}

fn marks_output_fields(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let is_marker = trimmed.contains(MARKER_PROVISIONAL) || trimmed.contains(MARKER_BEST_GUESS);
        if !is_marker || !trimmed.starts_with("//") {
            continue;
        }
        // Marker must sit immediately above `const OUTPUT_FIELDS` (allowing
        // continuation comment lines of a multi-line Best-guess note).
        for follow in lines.iter().skip(i + 1) {
            let f = follow.trim_start();
            if f.starts_with("//") {
                continue;
            }
            return f.starts_with("const OUTPUT_FIELDS");
        }
    }
    false
}

fn extract_reference(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("const REFERENCE:")
            .or_else(|| line.strip_prefix("const REFERENCE :"))
        else {
            continue;
        };
        let Some(start) = rest.find('"') else {
            continue;
        };
        let rest = &rest[start + 1..];
        let end = rest.find('"')?;
        return Some(rest[..end].to_owned());
    }
    None
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>, Fail> {
    let root_mod = root.join("mod.rs");
    if !root_mod.is_file() {
        // Flat test fixtures (and other non-crate trees) have no mod graph.
        return all_rust_files(root);
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    visit_mod(&root_mod, root, &mut out, &mut seen)?;
    out.sort();
    Ok(out)
}

fn all_rust_files(root: &Path) -> Result<Vec<PathBuf>, Fail> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Fail> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
        Ok(())
    }
    if root.is_dir() {
        walk(root, &mut out)?;
    }
    out.sort();
    Ok(out)
}

fn visit_mod(
    mod_file: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<(), Fail> {
    if !seen.insert(mod_file.to_path_buf()) {
        return Ok(());
    }
    out.push(mod_file.to_path_buf());
    let text = fs::read_to_string(mod_file)?;
    for name in mod_names(&text) {
        let as_file = dir.join(format!("{name}.rs"));
        let nested_mod = dir.join(&name).join("mod.rs");
        if nested_mod.is_file() {
            visit_mod(&nested_mod, &dir.join(&name), out, seen)?;
        } else if as_file.is_file() {
            if seen.insert(as_file.clone()) {
                out.push(as_file.clone());
                let nested_text = fs::read_to_string(&as_file)?;
                for nested_name in mod_names(&nested_text) {
                    let nested_file = dir.join(format!("{nested_name}.rs"));
                    let nested_dir_mod = dir.join(&nested_name).join("mod.rs");
                    if nested_dir_mod.is_file() {
                        visit_mod(&nested_dir_mod, &dir.join(&nested_name), out, seen)?;
                    } else if nested_file.is_file() && seen.insert(nested_file.clone()) {
                        out.push(nested_file);
                    }
                }
            }
        }
    }
    Ok(())
}

fn mod_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let rest = line
            .strip_prefix("pub mod ")
            .or_else(|| line.strip_prefix("mod "));
        let Some(rest) = rest else {
            continue;
        };
        let Some(name) = rest.strip_suffix(';') else {
            continue;
        };
        let name = name.trim();
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            names.push(name.to_owned());
        }
    }
    names
}

pub fn load_allowlist(path: &Path) -> Result<BTreeSet<String>, Fail> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("read allowlist {}: {e}", path.display()))?;
    let mut set = BTreeSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.contains(char::is_whitespace) {
            return Err(format!(
                "{}:{}: allowlist entries must be a single catalog ref (got {line:?})",
                path.display(),
                i + 1
            )
            .into());
        }
        if !set.insert(line.to_owned()) {
            return Err(format!(
                "{}:{}: duplicate allowlist entry {line}",
                path.display(),
                i + 1
            )
            .into());
        }
    }
    Ok(set)
}

pub fn cmd_list(workspace: &Path) -> Result<(), Fail> {
    let refs = scan_provisional_refs(&providers_dir(workspace))?;
    for r in &refs {
        println!("{r}");
    }
    eprintln!("{} provisional/best-guess ref(s)", refs.len());
    Ok(())
}

/// Fail when any Provisional/Best-guess ref is missing from the allowlist.
pub fn cmd_check(workspace: &Path) -> Result<(), Fail> {
    let found = scan_provisional_refs(&providers_dir(workspace))?;
    let allow = load_allowlist(&allowlist_path(workspace))?;
    let mut unknown: Vec<&str> = found
        .iter()
        .filter(|r| !allow.contains(r.as_str()))
        .map(String::as_str)
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        eprintln!("error: Provisional until / Best-guess markers outside the allowlist:");
        for r in &unknown {
            eprintln!("  {r}");
        }
        eprintln!(
            "\nPin via `mise run discover-apply`, or add the ref to\n  {}",
            allowlist_path(workspace).display()
        );
        std::process::exit(1);
    }
    println!(
        "provisional-check ok: {} marked ref(s), all allowlisted (allowlist size {})",
        found.len(),
        allow.len()
    );
    Ok(())
}

/// Drop a ref from the allowlist after a successful pin (keeps comments/header).
pub fn remove_from_allowlist(path: &Path, reference: &str) -> Result<bool, Fail> {
    let text = fs::read_to_string(path)?;
    let mut changed = false;
    let mut out = String::new();
    for line in text.lines() {
        if line.trim() == reference {
            changed = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if changed {
        fs::write(path, out)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn scan_finds_provisional_and_best_guess() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        let mut f = fs::File::create(&a).unwrap();
        writeln!(
            f,
            r#"
const REFERENCE: &'static str = "neon/postgres";
// Provisional until pinned by `mise run discover neon/postgres`.
const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[("DATABASE_URL", "database_url", true)];
"#
        )
        .unwrap();
        let b = dir.path().join("b.rs");
        let mut f = fs::File::create(&b).unwrap();
        writeln!(
            f,
            r#"
const REFERENCE: &'static str = "cloudflare/hyperdrive";
// Best-guess; unverified (needs origin).
// Still unpinned.
const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[("ID", "id", true)];
"#
        )
        .unwrap();
        // Module prose must not count.
        let c = dir.path().join("c.rs");
        let mut f = fs::File::create(&c).unwrap();
        writeln!(
            f,
            "//! Output envelopes are provisional until pinned.\nconst REFERENCE: &'static str = \"clerk/app\";"
        )
        .unwrap();

        let refs = scan_provisional_refs(dir.path()).unwrap();
        assert_eq!(
            refs,
            vec![
                "cloudflare/hyperdrive".to_owned(),
                "neon/postgres".to_owned()
            ]
        );
    }

    #[test]
    fn check_rejects_unknown_marker() {
        let dir = tempfile::tempdir().unwrap();
        let providers = dir.path().join("providers");
        fs::create_dir_all(&providers).unwrap();
        fs::write(
            providers.join("x.rs"),
            r#"
const REFERENCE: &'static str = "neon/postgres";
// Provisional until pinned.
const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[];
"#,
        )
        .unwrap();
        let allow = dir.path().join("allow.txt");
        fs::write(&allow, "# empty on purpose\n").unwrap();
        let found = scan_provisional_refs(&providers).unwrap();
        let set = load_allowlist(&allow).unwrap();
        assert!(found.iter().any(|r| !set.contains(r)));
    }

    #[test]
    fn scan_skips_undeclared_held_modules() {
        let dir = tempfile::tempdir().unwrap();
        let providers = dir.path();
        fs::write(providers.join("mod.rs"), "pub mod live;\n").unwrap();
        fs::write(
            providers.join("live.rs"),
            r#"
const REFERENCE: &'static str = "neon/postgres";
const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[];
"#,
        )
        .unwrap();
        fs::create_dir_all(providers.join("held")).unwrap();
        fs::write(
            providers.join("held/mod.rs"),
            "//! HELD — not registered.\npub mod service;\n",
        )
        .unwrap();
        fs::write(
            providers.join("held/service.rs"),
            r#"
const REFERENCE: &'static str = "algolia/application";
// Provisional until pinned.
const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[];
"#,
        )
        .unwrap();

        let refs = scan_provisional_refs(providers).unwrap();
        assert!(
            refs.is_empty(),
            "held undeclared module must be ignored: {refs:?}"
        );
    }
}
