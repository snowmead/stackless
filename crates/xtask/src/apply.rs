//! Apply discover-suggested OUTPUT_FIELDS into an integration module.
//!
//! Updates `OUTPUT_FIELDS`, syncs `Hostable::OUTPUTS`, rewrites hermetic
//! `provision_script` env keys, and strips Provisional/Best-guess comments.

use std::error::Error;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::provisional::{self, providers_dir};

type Fail = Box<dyn Error>;

/// Env-var prefix for a catalog provider segment (`wordpress.com` → `WORDPRESS_COM`).
pub fn catalog_provider_env_prefix(provider: &str) -> String {
    provider.to_ascii_uppercase().replace(['.', '-'], "_")
}

/// Structured pin payload (discover `--json` stdout, or a hand-edited file).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinSpec {
    pub reference: String,
    pub fields: Vec<OutputField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputField {
    /// Env-var suffix Stripe appends after `{RESOURCE}_` / `{PROVIDER}_`.
    pub env: String,
    /// Interpolation key (`integrations.<name>.<output>`).
    pub output: String,
    pub required: bool,
}

impl PinSpec {
    pub fn from_reader(mut r: impl Read) -> Result<Self, Fail> {
        let mut buf = String::new();
        r.read_to_string(&mut buf)?;
        Self::parse(&buf)
    }

    pub fn parse(text: &str) -> Result<Self, Fail> {
        let trimmed = text.trim();
        if trimmed.starts_with('{') {
            return Ok(serde_json::from_str(trimmed)?);
        }
        // Accept discover's human stdout (suggested OUTPUT_FIELDS block).
        parse_discover_stdout(trimmed)
    }
}

fn parse_discover_stdout(text: &str) -> Result<PinSpec, Fail> {
    let mut reference = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("output fields for ") {
            reference = Some(rest.trim_end_matches(':').trim().to_owned());
            break;
        }
        if let Some(rest) = line.strip_prefix("discovering ") {
            let r = rest.split_whitespace().next().unwrap_or_default();
            if !r.is_empty() {
                reference = Some(r.to_owned());
            }
        }
    }
    let reference = reference.ok_or(
        "could not find catalog reference in input (pass JSON with \"reference\", or discover stdout)",
    )?;

    let mut fields = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // `("SUFFIX", "output", true),`
        if !line.starts_with('(') {
            continue;
        }
        let Some(field) = parse_tuple_line(line) else {
            continue;
        };
        fields.push(field);
    }
    if fields.is_empty() {
        return Err("no OUTPUT_FIELDS tuples found in input".into());
    }
    Ok(PinSpec { reference, fields })
}

fn parse_tuple_line(line: &str) -> Option<OutputField> {
    let line = line.trim().trim_end_matches(',').trim();
    let line = line.strip_prefix('(')?.strip_suffix(')')?;
    let mut parts = vec![];
    let mut cur = String::new();
    let mut in_str = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                in_str = !in_str;
                cur.push(ch);
            }
            ',' if !in_str => {
                parts.push(cur.trim().to_owned());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_owned());
    }
    if parts.len() != 3 {
        return None;
    }
    let env = unquote(&parts[0])?;
    let output = unquote(&parts[1])?;
    let required = match parts[2].as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    Some(OutputField {
        env,
        output,
        required,
    })
}

fn unquote(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s.strip_prefix('"')?.strip_suffix('"')?;
    Some(s.to_owned())
}

pub fn find_module(providers: &Path, reference: &str) -> Result<PathBuf, Fail> {
    let needle = format!("\"{reference}\"");
    let mut matches = Vec::new();
    for path in rust_module_files(providers)? {
        let text = fs::read_to_string(&path)?;
        if text.lines().any(|l| {
            let l = l.trim();
            l.starts_with("const REFERENCE:") && l.contains(&needle)
        }) {
            matches.push(path);
        }
    }
    match matches.len() {
        0 => Err(format!("no integration module with REFERENCE = {reference:?}").into()),
        1 => Ok(matches.remove(0)),
        _ => Err(format!(
            "ambiguous REFERENCE {reference:?}: {}",
            matches
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into()),
    }
}

fn rust_module_files(root: &Path) -> Result<Vec<PathBuf>, Fail> {
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
    walk(root, &mut out)?;
    Ok(out)
}

pub fn apply_to_source(src: &str, spec: &PinSpec) -> Result<String, Fail> {
    let prefix = extract_provider_prefix(src).ok_or(
        "could not find const PROVIDER_PREFIX in module (needed to rewrite provision_script keys)",
    )?;
    let mut out = strip_provisional_comments(src);
    out = replace_outputs(&out, spec)?;
    out = replace_output_fields(&out, spec)?;
    out = rewrite_provision_script_keys(&out, &prefix, spec)?;
    Ok(out)
}

fn extract_provider_prefix(src: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("const PROVIDER_PREFIX:") else {
            continue;
        };
        let start = rest.find('"')?;
        let rest = &rest[start + 1..];
        let end = rest.find('"')?;
        return Some(rest[..end].to_owned());
    }
    None
}

fn strip_provisional_comments(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut drop = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let is_marker = (trimmed.contains("Provisional until") || trimmed.contains("Best-guess"))
            && trimmed.starts_with("//");
        if !is_marker {
            continue;
        }
        // Confirm this comment block leads to OUTPUT_FIELDS.
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim_start().starts_with("//") {
            j += 1;
        }
        if j < lines.len() && lines[j].trim_start().starts_with("const OUTPUT_FIELDS") {
            drop[i] = true;
            for flag in drop.iter_mut().take(j).skip(i + 1) {
                *flag = true;
            }
        }
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if drop[i] {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // Preserve missing final newline only if source lacked one.
    if !src.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn replace_outputs(src: &str, spec: &PinSpec) -> Result<String, Fail> {
    let names: Vec<String> = spec
        .fields
        .iter()
        .map(|f| format!("{:?}", f.output))
        .collect();
    let new_arr = format!("&[{}]", names.join(", "));
    replace_const_array(src, "const OUTPUTS:", &new_arr)
}

fn replace_output_fields(src: &str, spec: &PinSpec) -> Result<String, Fail> {
    let mut tuples = Vec::new();
    for f in &spec.fields {
        tuples.push(format!("({:?}, {:?}, {})", f.env, f.output, f.required));
    }
    let new_arr = if tuples.len() == 1 {
        format!("&[{}]", tuples[0])
    } else {
        let body = tuples
            .iter()
            .map(|t| format!("        {t},"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("&[\n{body}\n    ]")
    };
    replace_const_array(src, "const OUTPUT_FIELDS:", &new_arr)
        .map_err(|e| format!("{e} (shared OUTPUT_FIELDS const? edit by hand)").into())
}

/// Replace `const NAME: ... = <old>;` where the value is `&[...]`.
fn replace_const_array(src: &str, const_prefix: &str, new_value: &str) -> Result<String, Fail> {
    let Some(const_start) = src.find(const_prefix) else {
        return Err(format!("module has no {const_prefix}").into());
    };
    let after = &src[const_start..];
    let Some(eq) = after.find('=') else {
        return Err(format!("{const_prefix} has no '='").into());
    };
    let value_start = const_start + eq + 1;
    let mut i = value_start;
    while i < src.len() && src.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    if !src[i..].starts_with("&[") {
        return Err(format!("{const_prefix} value is not an inline &[...] array").into());
    }
    let end = match_bracket_array(src, i)? + 1; // position of `]`
    // Skip trailing whitespace then require `;`
    let mut j = end;
    while j < src.len() && src.as_bytes()[j].is_ascii_whitespace() {
        j += 1;
    }
    if j >= src.len() || src.as_bytes()[j] != b';' {
        return Err(format!("{const_prefix} array not followed by ';'").into());
    }
    let mut out = String::with_capacity(src.len());
    out.push_str(&src[..value_start]);
    out.push(' ');
    out.push_str(new_value);
    out.push_str(&src[j..]); // from `;`
    Ok(out)
}

fn match_bracket_array(src: &str, start: usize) -> Result<usize, Fail> {
    // start points at `&`
    let bytes = src.as_bytes();
    let mut i = start;
    if i + 1 >= bytes.len() || &src[i..i + 2] != "&[" {
        return Err("expected &[".into());
    }
    i += 2;
    let mut depth = 1i32;
    let mut in_str = false;
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err("unclosed &[...] in const array".into())
}

fn rewrite_provision_script_keys(src: &str, prefix: &str, spec: &PinSpec) -> Result<String, Fail> {
    let Some(call) = src.find("provision_script(") else {
        // Some modules may lack hermetic tests — pin the envelope anyway.
        return Ok(src.to_owned());
    };
    // Find the json!({ ... }) argument that carries PREFIX_ keys.
    let search_from = call;
    let window = &src[search_from..];
    let Some(json_rel) = window.find("serde_json::json!(") else {
        return Ok(src.to_owned());
    };
    let json_start = search_from + json_rel;
    let after_macro = json_start + "serde_json::json!".len();
    let mut i = after_macro;
    while i < src.len() && src.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= src.len() || src.as_bytes()[i] != b'(' {
        return Ok(src.to_owned());
    }
    let close = match_paren(src, i)?;
    let inner = &src[i + 1..close];
    // Only rewrite object literals that look like env maps for this prefix.
    if !inner.contains(&format!("\"{prefix}_")) && !inner.contains('{') {
        return Ok(src.to_owned());
    }
    let new_obj = format_env_json(prefix, spec);
    let mut out = String::new();
    out.push_str(&src[..i + 1]);
    out.push_str(&new_obj);
    out.push_str(&src[close..]);
    Ok(out)
}

fn format_env_json(prefix: &str, spec: &PinSpec) -> String {
    let mut parts = Vec::new();
    for f in &spec.fields {
        let key = format!("{prefix}_{}", f.env);
        let val = format!("val_{}", f.output);
        parts.push(format!("{key:?}: {val:?}"));
    }
    format!("{{{}}}", parts.join(", "))
}

fn match_paren(src: &str, start: usize) -> Result<usize, Fail> {
    let bytes = src.as_bytes();
    if bytes[start] != b'(' {
        return Err("expected '('".into());
    }
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err("unclosed '('".into())
}

pub struct ApplyArgs {
    pub workspace: PathBuf,
    pub input: Option<PathBuf>,
    pub dry_run: bool,
    pub keep_allowlist: bool,
}

pub fn cmd_apply(args: ApplyArgs) -> Result<(), Fail> {
    let spec = match &args.input {
        Some(path) if path.as_os_str() != "-" => PinSpec::parse(&fs::read_to_string(path)?)?,
        _ => PinSpec::from_reader(io::stdin())?,
    };
    if spec.fields.is_empty() {
        return Err("pin spec has empty fields".into());
    }
    let path = find_module(&providers_dir(&args.workspace), &spec.reference)?;
    let before = fs::read_to_string(&path)?;
    let after = apply_to_source(&before, &spec)?;
    if before == after {
        eprintln!(
            "no textual change for {} ({})",
            spec.reference,
            path.display()
        );
    } else if args.dry_run {
        println!("--- dry-run {} ({}) ---", spec.reference, path.display());
        print_section_diff("OUTPUTS", &before, &after, "const OUTPUTS:");
        print_section_diff("OUTPUT_FIELDS", &before, &after, "const OUTPUT_FIELDS:");
        print_provision_script_diff(&before, &after);
        if (before.contains("Provisional until") || before.contains("Best-guess"))
            && !after.contains("Provisional until")
            && !after.contains("Best-guess")
        {
            println!("- stripped Provisional until / Best-guess comment(s)");
        }
    } else {
        fs::write(&path, &after)?;
        println!("wrote {}", path.display());
        if !args.keep_allowlist {
            let allow = provisional::allowlist_path(&args.workspace);
            if provisional::remove_from_allowlist(&allow, &spec.reference)? {
                println!("removed {} from allowlist", spec.reference);
            }
        }
    }
    // Always show the resolved fields for review (required flags may need edits).
    println!(
        "\npinned {} fields for {}:",
        spec.fields.len(),
        spec.reference
    );
    for f in &spec.fields {
        println!(
            "  ({:?}, {:?}, {}){}",
            f.env,
            f.output,
            f.required,
            if f.required { "" } else { "  // optional" }
        );
    }
    eprintln!(
        "\nnote: discover marks only the first field required — edit `required` by hand if needed, then re-apply."
    );
    Ok(())
}

fn print_section_diff(label: &str, before: &str, after: &str, prefix: &str) {
    let b = const_assignment(before, prefix).unwrap_or("(missing)");
    let a = const_assignment(after, prefix).unwrap_or("(missing)");
    if b == a {
        println!("{label}: unchanged");
        return;
    }
    println!("{label}:");
    println!("- {b}");
    println!("+ {a}");
}

fn const_assignment<'a>(src: &'a str, prefix: &str) -> Option<&'a str> {
    let start = src.find(prefix)?;
    let from_eq = src[start..].find('=')? + start;
    let mut i = from_eq + 1;
    while i < src.len() && src.as_bytes()[i].is_ascii_whitespace() {
        i += 1;
    }
    let end = match_bracket_array(src, i).ok()? + 1;
    Some(src[start..end].trim())
}

fn json_macro_object(src: &str) -> Option<&str> {
    let start = src.find("serde_json::json!(")?;
    let open = src[start..].find('(')? + start;
    let close = match_paren(src, open).ok()?;
    Some(src[open + 1..close].trim())
}

fn print_provision_script_diff(before: &str, after: &str) {
    match (json_macro_object(before), json_macro_object(after)) {
        (Some(b), Some(a)) if b != a => {
            println!("provision_script env:");
            println!("- {b}");
            println!("+ {a}");
        }
        (Some(_), Some(_)) => println!("provision_script env: unchanged"),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"//! sample
impl Hostable for NeonPostgres {
    const OUTPUTS: &'static [&'static str] = &["database_url", "host"];
}
impl FamilyResource for NeonPostgres {
    const PROVIDER_PREFIX: &'static str = "NEON";
    // Provisional until pinned by `mise run discover neon/postgres`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("DATABASE_URL", "database_url", true),
        ("HOST", "host", false),
    ];
}
#[cfg(test)]
mod tests {
    async fn provision_records_outputs() {
        let runner = test_support::provision_script(
            CATALOG_ENVELOPE,
            serde_json::json!({"NEON_DATABASE_URL": "val_database_url", "NEON_HOST": "val_host"}),
            0,
        );
    }
}
"#;

    #[test]
    fn apply_rewrites_fields_outputs_script_and_strips_marker() {
        let spec = PinSpec {
            reference: "neon/postgres".into(),
            fields: vec![
                OutputField {
                    env: "DATABASE_URL".into(),
                    output: "database_url".into(),
                    required: true,
                },
                OutputField {
                    env: "BRANCH_ID".into(),
                    output: "branch_id".into(),
                    required: false,
                },
            ],
        };
        let out = apply_to_source(SAMPLE, &spec).unwrap();
        assert!(!out.contains("Provisional until"));
        assert!(out.contains(
            r#"const OUTPUTS: &'static [&'static str] = &["database_url", "branch_id"];"#
        ));
        assert!(out.contains(r#"("BRANCH_ID", "branch_id", false)"#));
        assert!(out.contains(
            r#"serde_json::json!({"NEON_DATABASE_URL": "val_database_url", "NEON_BRANCH_ID": "val_branch_id"})"#
        ));
        assert!(!out.contains("NEON_HOST"));
    }

    #[test]
    fn catalog_provider_env_prefix_maps_dots_and_hyphens() {
        assert_eq!(
            catalog_provider_env_prefix("wordpress.com"),
            "WORDPRESS_COM"
        );
        assert_eq!(catalog_provider_env_prefix("neon"), "NEON");
        assert_eq!(
            catalog_provider_env_prefix("laravel-cloud"),
            "LARAVEL_CLOUD"
        );
    }

    #[test]
    fn parse_json_and_discover_stdout() {
        let json = r#"{"reference":"neon/postgres","fields":[{"env":"DATABASE_URL","output":"database_url","required":true}]}"#;
        let a = PinSpec::parse(json).unwrap();
        assert_eq!(a.reference, "neon/postgres");
        assert_eq!(a.fields.len(), 1);

        let human = r#"
discovering neon/postgres (paid=false) via throwaway env disco-1...
output fields for neon/postgres:
  DATABASE_URL  ->  database_url
suggested OUTPUT_FIELDS (mark required as appropriate):
    ("DATABASE_URL", "database_url", true),
"#;
        let b = PinSpec::parse(human).unwrap();
        assert_eq!(b, a);
    }
}
