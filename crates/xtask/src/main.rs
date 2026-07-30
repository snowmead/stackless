//! Provider-onboarding dev tooling.
//!
//! - `catalog [<provider>]` — list catalog services + config schemas + pricing
//!   (offline, from the committed catalog fixture).
//! - `discover <reference>` — provision a resource into a throwaway environment,
//!   dump its real output env vars (the credential envelope the catalog does not
//!   describe), then tear down. Live: needs `STRIPE_API_KEY` + the `stripe` CLI.
//! - `new-integration <reference>` — scaffold a provider module from the schema.

use std::error::Error;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use serde_json::Value;
use stackless_stripe_projects::catalog::{Catalog, ServiceDetail};
use stackless_stripe_projects::stripe::TokioRunner;
use stackless_stripe_projects::{StripeProjects, project};

type Fail = Box<dyn Error>;

/// The committed catalog fixture (the same data the gap tests validate against).
const CATALOG_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../stackless-stripe-projects/tests/fixtures/catalog.json"
));

#[derive(Parser)]
#[command(about = "stackless provider-onboarding tooling")]
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
    Discover {
        /// Catalog reference, e.g. `cloudflare/kv`.
        reference: String,
        /// `--config` JSON for the resource (default `{}`).
        #[arg(long)]
        config: Option<String>,
        /// Directory with a linked Stripe Projects context (default: cwd).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Scaffold a new integration module from the catalog schema.
    NewIntegration {
        /// Catalog reference, e.g. `cloudflare/kv`.
        reference: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Fail> {
    match Cli::parse().command {
        Command::Catalog { provider } => cmd_catalog(provider.as_deref()),
        Command::Discover {
            reference,
            config,
            dir,
        } => cmd_discover(&reference, config.as_deref(), &dir).await,
        Command::NewIntegration { reference } => cmd_new_integration(&reference),
    }
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

async fn cmd_discover(reference: &str, config: Option<&str>, dir: &Path) -> Result<(), Fail> {
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
    let provider_prefix = reference
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
        .replace('-', "_");

    println!("discovering {reference} (paid={paid}) via throwaway env {env_name}...");
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
        println!("no output env vars found for {reference} (resource {attached_name}).");
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
    let provider_prefix = provider.to_ascii_uppercase().replace('-', "_");
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
