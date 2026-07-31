//! Provider-onboarding dev tooling.
//!
//! - `catalog [<provider>]` — list catalog services + config schemas + pricing
//!   (offline, from the committed catalog fixture).
//! - `discover <reference>` — provision a resource into a throwaway environment,
//!   dump its real output env vars (the credential envelope the catalog does not
//!   describe), then tear down. Live: needs `STRIPE_API_KEY` + the `stripe` CLI.
//! - `discover-apply` — write discover-suggested OUTPUT_FIELDS into the
//!   integration module (stdout/file; `--dry-run` supported).
//! - `provisional-list` / `provisional-check` — inventory unpinned envelopes and
//!   gate them against the committed allowlist.
//! - `catalog-orphans` — fail if any catalog deployable is unowned (delegates to
//!   `scripts/generate_catalog_integrations.py --check-orphans`).
//! - `new-integration <reference>` — scaffold a provider module from the schema.

mod apply;
mod provisional;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use clap::{Parser, Subcommand};
use serde_json::Value;
use stackless_stripe_projects::catalog::{Catalog, ServiceDetail};
use stackless_stripe_projects::stripe::TokioRunner;
use stackless_stripe_projects::{StripeProjects, project};

use apply::{ApplyArgs, PinSpec};
use provisional::{cmd_check as provisional_check, cmd_list as provisional_list};

type Fail = Box<dyn Error>;

/// Exit when discover needs a human (`stripe projects link`, `--config`, billing).
const EXIT_NEEDS_HUMAN: u8 = 2;

/// The committed catalog fixture (the same data the gap tests validate against).
const CATALOG_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../stackless-stripe-projects/tests/fixtures/catalog.json"
));

#[derive(Parser)]
#[command(
    about = "stackless provider-onboarding tooling",
    long_about = "Provider onboarding lever: catalog / catalog-orphans / discover / discover-apply / provisional-*.\n\n\
Preflight before live discover:\n\
  • `stripe projects init` in a fixture dir (or any linked project context)\n\
  • `stripe projects link <provider>` once per Stripe account (human OAuth)\n\
  • pass `--config '{...}'` when the catalog schema has required fields\n\n\
Cloudflare rate limit: ~2 provisions per ~22 minutes — space discover/smoke work.\n\
discover-apply accepts discover `--json` stdout or a hand-edited JSON file; use\n\
`--dry-run` to preview. Exit code 2 means link/config/billing needs a human."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List catalog services + config schemas + pricing (offline).
    Catalog {
        /// Provider name (e.g. `cloudflare`); omit to list all.
        provider: Option<String>,
    },
    /// Provision a resource into a throwaway env and dump its output env vars.
    #[command(long_about = "Live-pin a catalog reference's credential envelope.\n\n\
Preflight:\n\
  1. Working dir (or `--dir`) must be an initialized Stripe project \
     (`stripe projects init <stack>`).\n\
  2. Provider must be linked: `stripe projects link <provider>` (one-time human OAuth).\n\
  3. If the service schema requires fields, pass `--config '{\"key\":\"val\",...}'`.\n\
  4. Cloudflare: space attempts (~2 provisions / ~22 min) or discover will 429.\n\n\
Exit codes: 0 ok · 1 hard failure · 2 needs human link/config/billing.\n\
Pass `--json` to emit a PinSpec for `discover-apply`.")]
    Discover {
        /// Catalog reference, e.g. `cloudflare/kv`.
        reference: String,
        /// `--config` JSON for the resource (default `{}`). Required when the
        /// catalog schema has required properties (e.g. Hyperdrive origin DB).
        #[arg(long)]
        config: Option<String>,
        /// Directory with a linked Stripe Projects context (default: cwd).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Emit a JSON PinSpec suitable for piping into `discover-apply`.
        #[arg(long)]
        json: bool,
    },
    /// Apply discover-suggested OUTPUT_FIELDS into an integration module.
    #[command(
        name = "discover-apply",
        long_about = "Write a PinSpec into the matching providers/ module:\n\
  • replace OUTPUT_FIELDS\n\
  • sync Hostable::OUTPUTS\n\
  • rewrite hermetic provision_script env keys\n\
  • strip Provisional until / Best-guess comments\n\
  • drop the ref from the provisional allowlist (unless `--keep-allowlist`)\n\n\
Input: JSON PinSpec on stdin (default), or `--file path` / `--file -`.\n\
Produce JSON via `mise run discover <ref> -- --json --dir ...` or hand-edit.\n\n\
Preflight for live discover upstream of this command:\n\
  • `stripe projects link <provider>` (human OAuth, once per account)\n\
  • `--config` when the catalog schema requires it\n\
  • Cloudflare: ~2 provisions per ~22 minutes — space live pins\n\n\
`--dry-run` prints a line diff and does not write. Discover's first-field\n\
`required: true` heuristic is often wrong — edit the JSON before apply."
    )]
    DiscoverApply {
        /// Read PinSpec JSON from this path (`-` = stdin).
        #[arg(long, short = 'f')]
        file: Option<PathBuf>,
        /// Print the rewrite without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Do not remove the ref from provisional-allowlist.txt after apply.
        #[arg(long)]
        keep_allowlist: bool,
    },
    /// List catalog refs still marked Provisional until / Best-guess (offline).
    #[command(name = "provisional-list")]
    ProvisionalList,
    /// Fail if Provisional/Best-guess refs appear outside the allowlist.
    #[command(name = "provisional-check")]
    ProvisionalCheck,
    /// Fail if any catalog deployable is unowned (offline).
    #[command(name = "catalog-orphans")]
    CatalogOrphans,
    /// Scaffold a new integration module from the catalog schema.
    NewIntegration {
        /// Catalog reference, e.g. `cloudflare/kv`.
        reference: String,
    },
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

#[tokio::main]
async fn main() -> ExitCode {
    let workspace = workspace_root();
    let command = Cli::parse().command;
    let is_discover = matches!(command, Command::Discover { .. });
    let result = match command {
        Command::Catalog { provider } => cmd_catalog(provider.as_deref()),
        Command::Discover {
            reference,
            config,
            dir,
            json,
        } => cmd_discover(&reference, config.as_deref(), &dir, json).await,
        Command::DiscoverApply {
            file,
            dry_run,
            keep_allowlist,
        } => apply::cmd_apply(ApplyArgs {
            workspace,
            input: file,
            dry_run,
            keep_allowlist,
        }),
        Command::ProvisionalList => provisional_list(&workspace),
        Command::ProvisionalCheck => provisional_check(&workspace),
        Command::CatalogOrphans => cmd_catalog_orphans(&workspace),
        Command::NewIntegration { reference } => cmd_new_integration(&reference),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            if is_discover && needs_human(&err.to_string()) {
                eprintln!(
                    "hint: exit {EXIT_NEEDS_HUMAN} — run `stripe projects link <provider>` \
                     and/or pass `--config`, then retry (Cloudflare: space ~22 min)."
                );
                ExitCode::from(EXIT_NEEDS_HUMAN)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn cmd_catalog_orphans(workspace: &Path) -> Result<(), Fail> {
    let script = workspace.join("scripts/generate_catalog_integrations.py");
    let status = ProcessCommand::new("python3")
        .arg(&script)
        .arg("--check-orphans")
        .current_dir(workspace)
        .status()?;
    if !status.success() {
        return Err("catalog orphan check failed".into());
    }
    Ok(())
}

fn needs_human(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    [
        "link",
        "pending_auth",
        "expired",
        "get started by running",
        "price_confirmation",
        "accept-tos",
        "not authenticated",
        "login",
        "oauth",
        "configuration",
        "required property",
        "invalid_config",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

fn cmd_catalog(provider: Option<&str>) -> Result<(), Fail> {
    let catalog = Catalog::from_json_envelope(CATALOG_FIXTURE)?;
    let mut services: Vec<&ServiceDetail> = catalog
        .services
        .iter()
        .filter(|s| provider.is_none_or(|p| s.provider_name.eq_ignore_ascii_case(p)))
        .collect();
    services.sort_by_key(|a| a.reference());
    if services.is_empty() {
        println!(
            "no services{}",
            provider.map(|p| format!(" for {p}")).unwrap_or_default()
        );
        return Ok(());
    }
    for service in services {
        println!(
            "{}  [{:?} / {:?}]  pricing: {:?}",
            service.reference(),
            service.kind,
            service.scope,
            service.pricing.kind
        );
        if let Some(schema) = &service.configuration_schema {
            for (name, prop) in &schema.properties {
                let req = if schema.required.contains(name) {
                    "required"
                } else {
                    "optional"
                };
                let enums = if prop.allowed.is_empty() {
                    String::new()
                } else {
                    let vals: Vec<&str> = prop.allowed.iter().filter_map(Value::as_str).collect();
                    format!("; enum: {}", vals.join(", "))
                };
                println!("    {name}  ({req}, {:?}{enums})", prop.prop_type);
            }
        }
    }
    Ok(())
}

async fn cmd_discover(
    reference: &str,
    config: Option<&str>,
    dir: &Path,
    json_out: bool,
) -> Result<(), Fail> {
    let config_value: Value = match config {
        Some(c) => serde_json::from_str(c)?,
        None => serde_json::json!({}),
    };
    let stripe = StripeProjects::new(TokioRunner, dir.to_path_buf());
    let catalog = stripe.catalog_for_reference(reference).await?;
    let service = catalog
        .lookup(reference)
        .ok_or_else(|| format!("reference {reference:?} not found in the live catalog"))?;
    let paid = service.requires_confirmation(&config_value);

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let env_name = format!("disco-{stamp}");
    let env_file = format!(".env.{env_name}");
    let resource = format!("disco-{}", reference.replace(['/', ':'], "-"));
    let provider = reference.split('/').next().unwrap_or_default();
    let provider_prefix = apply::catalog_provider_env_prefix(provider);

    if !json_out {
        println!("discovering {reference} (paid={paid}) via throwaway env {env_name}...");
    }
    stripe
        .json(&["env", "create", &env_name, "--output", &env_file, "--yes"])
        .await?;
    stripe.json(&["env", "use", &env_name]).await?;

    let provisioned =
        project::add_resource(&stripe, reference, &resource, &config_value, paid).await;
    if provisioned.is_ok() {
        let _ = stripe.json(&["env", "--pull", "--refresh", "--yes"]).await;
    }
    let env_text = std::fs::read_to_string(dir.join(&env_file)).unwrap_or_default();

    // Best-effort teardown before surfacing any provisioning error.
    let attached_name = provisioned
        .as_ref()
        .map(|added| added.name.clone())
        .unwrap_or_else(|_| resource.clone());
    let _ = project::remove_resource(&stripe, &attached_name).await;
    let _ = stripe.json(&["env", "use", "default"]).await;
    let _ = stripe.json(&["env", "delete", &env_name, "--yes"]).await;
    let _ = std::fs::remove_file(dir.join(&env_file));

    provisioned.map_err(|err| format!("provisioning {reference} failed: {err}"))?;

    // Stripe names a resource's vars `{RESOURCE}_{SUFFIX}` (or `{PROVIDER}_{SUFFIX}`
    // when unambiguous) — strip either prefix to recover the field suffix.
    // Use the attached/reused local name, not the originally requested disco-* name.
    let resource_prefix = attached_name.to_ascii_uppercase().replace('-', "_");
    let mut fields: Vec<String> = Vec::new();
    for line in env_text.lines() {
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let suffix = key
            .strip_prefix(&format!("{resource_prefix}_"))
            .or_else(|| key.strip_prefix(&format!("{provider_prefix}_")));
        if let Some(suffix) = suffix {
            fields.push(suffix.to_owned());
        }
    }
    if fields.is_empty() {
        if json_out {
            let spec = PinSpec {
                reference: reference.to_owned(),
                fields: vec![],
            };
            println!("{}", serde_json::to_string_pretty(&spec)?);
        } else {
            println!("no output env vars found for {reference} (resource {attached_name}).");
        }
        return Ok(());
    }

    let spec = PinSpec {
        reference: reference.to_owned(),
        fields: fields
            .iter()
            .enumerate()
            .map(|(i, f)| apply::OutputField {
                env: f.clone(),
                output: f.to_ascii_lowercase(),
                required: i == 0,
            })
            .collect(),
    };

    if json_out {
        println!("{}", serde_json::to_string_pretty(&spec)?);
        return Ok(());
    }

    println!("\noutput fields for {reference}:");
    for f in &fields {
        println!("  {f}  ->  {}", f.to_ascii_lowercase());
    }
    println!("\nsuggested OUTPUT_FIELDS (mark required as appropriate):");
    for (i, f) in fields.iter().enumerate() {
        let required = i == 0;
        println!("    ({:?}, {:?}, {required}),", f, f.to_ascii_lowercase());
    }
    println!(
        "\n# pipe into apply:\n#   mise run discover {reference} -- --json --dir <fixture> \\\n#     | mise run discover-apply -- --dry-run"
    );
    Ok(())
}

fn cmd_new_integration(reference: &str) -> Result<(), Fail> {
    let catalog = Catalog::from_json_envelope(CATALOG_FIXTURE)?;
    let service = catalog
        .lookup(reference)
        .ok_or_else(|| format!("reference {reference:?} not found in the catalog"))?;
    let (provider, svc) = reference
        .split_once('/')
        .ok_or("reference must be <provider>/<service>")?;
    let provider_key = format!("{provider}-{}", svc.replace(':', "-")); // provider = "cloudflare-kv"
    let resource_kind = format!("integration-{provider_key}");
    let provider_prefix = apply::catalog_provider_env_prefix(provider);
    let type_name = format!("{}{}", camel(provider), camel(svc));
    let config_type = format!("{type_name}Config");

    let schema = service.configuration_schema.as_ref();
    let mut struct_fields = String::new();
    let mut build_fields = String::new();
    let mut validate = String::new();
    if let Some(schema) = schema {
        for (key, prop) in &schema.properties {
            let required = schema.required.contains(key);
            use stackless_stripe_projects::catalog::PropertyType::*;
            match (&prop.prop_type, required) {
                (String, true) => {
                    struct_fields.push_str(&format!("    pub {key}: std::string::String,\n"));
                    build_fields.push_str(&format!(
                        "            {key}: super::interp_required(ctx, &config, \"{key}\")?,\n"
                    ));
                    validate.push_str(&format!(
                        "    registry::config_string(config, \"{key}\").map_err(|e| IntegrationError::ConfigInvalid {{ location: format!(\"integrations.{{name}}.{key}\"), detail: e.to_string() }})?;\n"
                    ));
                }
                (String, false) => {
                    struct_fields.push_str(&format!(
                        "    #[serde(skip_serializing_if = \"Option::is_none\")]\n    pub {key}: Option<std::string::String>,\n"
                    ));
                    build_fields.push_str(&format!(
                        "            {key}: super::interp_optional(ctx, &config, \"{key}\")?,\n"
                    ));
                }
                (Integer, true) => {
                    struct_fields.push_str(&format!("    pub {key}: i64,\n"));
                    build_fields.push_str(&format!(
                        "            {key}: super::int_required(ctx, &config, \"{key}\")?,\n"
                    ));
                }
                (other, req) => {
                    struct_fields.push_str(&format!(
                        "    // TODO: {key} ({other:?}, required={req}) — add field + build_config wiring\n"
                    ));
                }
            }
        }
    }

    println!(
        r#"//! {reference} integration (generated skeleton — run `xtask discover {reference}`
//! to pin OUTPUT_FIELDS, then add the registry row + `pub mod` shown at the bottom).

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CatalogResource;
use crate::error::IntegrationError;
use crate::hostable::{{ConfigScope, Hostable, IntegrationHosting}};
use crate::registry;

pub const RESOURCE_KIND: &str = "{resource_kind}";

#[derive(Debug, Serialize)]
pub struct {config_type} {{
{struct_fields}}}

impl CatalogService for {config_type} {{
    const REFERENCE: &'static str = "{reference}";
}}

#[derive(Debug)]
pub struct {type_name};

impl Hostable for {type_name} {{
    const PROVIDER: &'static str = "{provider_key}";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[/* fill from `xtask discover` */];
}}

impl CatalogResource for {type_name} {{
    type Config = {config_type};
    const PROVIDER_PREFIX: &'static str = "{provider_prefix}";
    // TODO: paste from `xtask discover {reference}` — (env suffix, output, required).
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<{config_type}, IntegrationError> {{
        let config = super::integration_config(ctx)?;
        Ok({config_type} {{
{build_fields}        }})
    }}
}}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {{
{validate}    Ok(())
}}

// === register (the one site) ===
// providers/mod.rs:        pub mod {module};
// registry.rs PROVIDERS:   provider_entry::<...::{type_name}>(...::validate_config, &...::{type_name}),
"#,
        module = svc.replace([':', '-'], "_"),
    );
    Ok(())
}

/// CamelCase a `provider`/`service` segment (`r2:bucket` -> `R2Bucket`).
fn camel(s: &str) -> std::string::String {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => std::string::String::new(),
            }
        })
        .collect()
}
