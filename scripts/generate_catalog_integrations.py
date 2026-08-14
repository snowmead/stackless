#!/usr/bin/env python3
"""Generate CatalogResource modules, or check catalog ownership (`--check-orphans`)."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROVIDERS = ROOT / "crates/stackless-integrations/src/providers"
CATALOG = json.loads(
    (ROOT / "crates/stackless-stripe-projects/tests/fixtures/catalog.json").read_text()
)

# Deployables we will never list or scaffold. Reasons are the product contract —
# `mise run catalog-orphans` fails if a catalog deployable is not registered,
# substrate-only hosting, or present here.
EXCL: dict[str, str] = {
    "cloudflare/containers": (
        "PRICE_CONFIRMATION_REQUIRED — unknown cost; not auto-provisioned"
    ),
    "cloudflare/registrar:domain": (
        "Non-refundable domain purchase; never in the leased lifecycle"
    ),
    "squarespace/domain": (
        "Non-refundable domain purchase; never in the leased lifecycle"
    ),
    "wordpress.com/domain": (
        "Non-refundable domain purchase; never in the leased lifecycle"
    ),
    "spaceship/domain": (
        "Non-refundable domain purchase; never in the leased lifecycle"
    ),
    "createos/project": (
        "Catalog orphan — no stackless surface; do not register or scaffold"
    ),
    # External Stripe/provider gates — held source, not registered until pin.
    # Tracking: #91–#97. Keep in sync with docs/ADDING-A-PROVIDER.md.
    "algolia/application": (
        "#91 — Missing Application plan; no catalog plan service to pre-add"
    ),
    "blaxel/agent-drive": (
        "#92 — Private preview 402 waitlist; blaxel/sandbox remains registered"
    ),
    "chroma/database": (
        "#93 — Missing from live Stripe catalog (Unknown provider)"
    ),
    "daytona/sandbox": (
        "#94 — Linkable but provision stalls pending with no credential envs"
    ),
    "heygen/api": (
        "#95 — Missing from live Stripe catalog filter (Unknown provider/service)"
    ),
    "privy/app": (
        "#96 — Missing from live Stripe providers index (Unknown provider)"
    ),
    "twilio/email": (
        "#97 — US-region accounts only (not_in_country / provider_failure)"
    ),
}

# Hosting refs owned by substrate crates (not CatalogResource integrations).
# Dual-registered refs (railway/hosting, cloudflare/workers, …) are covered by
# scanning integration REFERENCE consts instead.
SUBSTRATE_ONLY_HOSTING: frozenset[str] = frozenset(
    {
        "flyio/app",
        "netlify/project",
        "render/static-site",
        "render/web-service",
        "vercel/project",
    }
)

REFERENCE_RE = re.compile(
    r"const\s+REFERENCE:\s*&'static\s+str\s*=\s*\"([^\"]+)\""
)
MOD_RE = re.compile(r"^\s*(?:pub\s+)?mod\s+(\w+)\s*;", re.M)

SHORT_PROVIDER = {
    "auth0/client": "auth0",
    "workos/auth": "workos",
    "privy/app": "privy",
    "neon/postgres": "neon",
    "supabase/project": "supabase",
    "turso/database": "turso",
    "prisma/database": "prisma",
    "chroma/database": "chroma",
    "algolia/application": "algolia",
    "openrouter/api": "openrouter",
    "exa/api": "exa",
    "firecrawl/api": "firecrawl",
    "parallel/api": "parallel",
    "elevenlabs/tts": "elevenlabs",
    "heygen/api": "heygen",
    "e2b/sandbox": "e2b",
    "daytona/sandbox": "daytona",
    "browserbase/project": "browserbase",
    "runloop/sandbox": "runloop",
    "kernel/project": "kernel",
    "agentmail/api": "agentmail",
    "agentphone/number": "agentphone",
    "inngest/app": "inngest",
    "gitlab/project": "gitlab",
    "metronome/sandbox": "metronome",
    "supermemory/memory": "supermemory",
    "postalform/mail": "postalform",
    "shopify/store": "shopify",
    "wix/headless": "wix",
    "base44_projects/app": "base44",
    "wordpress.com/site": "wordpress-com",
    "amplitude/analytics": "amplitude",
    "mixpanel/analytics": "mixpanel",
    "posthog/analytics": "posthog",
    "sentry/project": "sentry",
    "sentry/seer": "sentry-seer",
    "render/postgres": "render-postgres",
    "railway/hosting": "railway-hosting",
    "railway/postgres": "railway-postgres",
    "railway/redis": "railway-redis",
    "railway/mongo": "railway-mongo",
    "railway/bucket": "railway-bucket",
    "upstash/redis": "upstash-redis",
    "upstash/qstash": "upstash-qstash",
    "upstash/search": "upstash-search",
    "upstash/vector": "upstash-vector",
    "planetscale/mysql": "planetscale-mysql",
    "planetscale/postgresql": "planetscale-postgresql",
    "clickhouse/clickhouse": "clickhouse",
    "clickhouse/postgres": "clickhouse-postgres",
    "blaxel/agent-drive": "blaxel-agent-drive",
    "blaxel/sandbox": "blaxel-sandbox",
    "huggingface/platform": "huggingface",
    "huggingface/bucket": "huggingface-bucket",
    "laravel_cloud/application": "laravel-cloud",
    "laravel_cloud/mysql": "laravel-cloud-mysql",
    "laravel_cloud/valkey": "laravel-cloud-valkey",
    "flyio/mpg": "flyio-mpg",
    "flyio/sprite": "flyio-sprite",
}

# Skip refs that already have adapters. Includes SHORT_PROVIDER keys so a regen
# does not overwrite hand-written family modules.
IMPLEMENTED = {
    "clerk/auth",
    "cloudflare/r2:bucket",
    "cloudflare/kv",
    "cloudflare/d1",
    "cloudflare/queues",
    "cloudflare/hyperdrive",
    "cloudflare/workers",
    "cloudflare/workers-ai",
    "cloudflare/browser-run",
    "render/web-service",
    "render/static-site",
    "vercel/project",
    "flyio/app",
    "netlify/project",
    *SHORT_PROVIDER,
}

OUTPUT_HINTS = {
    "neon/postgres": [("DATABASE_URL", "database_url", True), ("HOST", "host", False)],
    "supabase/project": [
        ("URL", "url", True),
        ("ANON_KEY", "anon_key", True),
        ("SERVICE_ROLE_KEY", "service_role_key", False),
    ],
    "turso/database": [("DATABASE_URL", "database_url", True), ("AUTH_TOKEN", "auth_token", True)],
    "prisma/database": [("DATABASE_URL", "database_url", True)],
    "auth0/client": [
        ("DOMAIN", "domain", True),
        ("CLIENT_ID", "client_id", True),
        ("CLIENT_SECRET", "client_secret", True),
    ],
    "workos/auth": [("API_KEY", "api_key", True), ("CLIENT_ID", "client_id", True)],
    "privy/app": [("APP_ID", "app_id", True), ("APP_SECRET", "app_secret", True)],
    "upstash/redis": [("REDIS_URL", "redis_url", True), ("REST_TOKEN", "rest_token", False)],
    "upstash/qstash": [("TOKEN", "token", True)],
    "upstash/search": [("REST_URL", "rest_url", True), ("REST_TOKEN", "rest_token", True)],
    "upstash/vector": [("REST_URL", "rest_url", True), ("REST_TOKEN", "rest_token", True)],
    "planetscale/mysql": [("DATABASE_URL", "database_url", True)],
    "planetscale/postgresql": [("DATABASE_URL", "database_url", True)],
    "clickhouse/clickhouse": [("CONNECTION_STRING", "connection_string", True)],
    "clickhouse/postgres": [("CONNECTION_STRING", "connection_string", True)],
    "chroma/database": [("API_KEY", "api_key", True)],
    "sentry/project": [
        ("AUTH_TOKEN", "auth_token", True),
        ("DSN", "dsn", True),
        ("ORG", "org", True),
        ("PROJECT", "project", True),
        ("URL", "url", True),
    ],
    "sentry/seer": [("AUTH_TOKEN", "auth_token", True)],
    "posthog/analytics": [("API_KEY", "api_key", True)],
    "amplitude/analytics": [("API_KEY", "api_key", True)],
    "mixpanel/analytics": [("TOKEN", "token", True)],
    "algolia/application": [("APP_ID", "app_id", True), ("API_KEY", "api_key", True)],
    "openrouter/api": [("API_KEY", "api_key", True)],
    "exa/api": [("API_KEY", "api_key", True)],
    "firecrawl/api": [("API_KEY", "api_key", True)],
    "parallel/api": [("API_KEY", "api_key", True)],
    "elevenlabs/tts": [("API_KEY", "api_key", True)],
    "heygen/api": [("API_KEY", "api_key", True)],
    "huggingface/platform": [("TOKEN", "token", True)],
    "huggingface/bucket": [("BUCKET_NAME", "bucket_name", True)],
    "inngest/app": [("EVENT_KEY", "event_key", True), ("SIGNING_KEY", "signing_key", False)],
    "e2b/sandbox": [("API_KEY", "api_key", True)],
    "daytona/sandbox": [("API_KEY", "api_key", True)],
    "browserbase/project": [("API_KEY", "api_key", True), ("PROJECT_ID", "project_id", True)],
    "blaxel/agent-drive": [("API_KEY", "api_key", True)],
    "blaxel/sandbox": [("API_KEY", "api_key", True)],
    "runloop/sandbox": [("API_KEY", "api_key", True)],
    "kernel/project": [("API_KEY", "api_key", True)],
    "agentmail/api": [("API_KEY", "api_key", True)],
    "agentphone/number": [("PHONE_NUMBER", "phone_number", True)],
    "railway/hosting": [("PROJECT_ID", "project_id", True)],
    "railway/postgres": [("DATABASE_URL", "database_url", True)],
    "railway/redis": [("REDIS_URL", "redis_url", True)],
    "railway/mongo": [("DATABASE_URL", "database_url", True)],
    "railway/bucket": [("BUCKET", "bucket", True)],
    "gitlab/project": [("PROJECT_ID", "project_id", True), ("WEB_URL", "web_url", False)],
    "laravel_cloud/application": [("APP_ID", "app_id", True)],
    "laravel_cloud/mysql": [("DATABASE_URL", "database_url", True)],
    "laravel_cloud/valkey": [("REDIS_URL", "redis_url", True)],
    "wordpress.com/site": [("SITE_URL", "site_url", True)],
    "base44_projects/app": [("APP_ID", "app_id", True)],
    "wix/headless": [("APP_ID", "app_id", True)],
    "postalform/mail": [("API_KEY", "api_key", True)],
    "metronome/sandbox": [("API_KEY", "api_key", True)],
    "supermemory/memory": [("API_KEY", "api_key", True)],
    "render/postgres": [("DATABASE_URL", "database_url", True)],
    "flyio/mpg": [("DATABASE_URL", "database_url", True)],
    "flyio/sprite": [("SPRITE_URL", "sprite_url", True)],
}


def camel(s: str) -> str:
    parts = re.split(r"[^a-zA-Z0-9]+", s)
    return "".join(p[:1].upper() + p[1:] for p in parts if p)


def family_dir(provider_name: str) -> str:
    special = {
        "Render": "render_db",
        "Flyio": "flyio",
        "WordPress.com": "wordpress_com",
        "Laravel_Cloud": "laravel_cloud",
        "Base44_Projects": "base44_projects",
        "KERNEL": "kernel",
        "HuggingFace": "huggingface",
        "OpenRouter": "openrouter",
        "AgentMail": "agentmail",
        "AgentPhone": "agentphone",
        "PlanetScale": "planetscale",
        "ClickHouse": "clickhouse",
        "PostalForm": "postalform",
        "ElevenLabs": "elevenlabs",
        "HeyGen": "heygen",
        "Firecrawl": "firecrawl",
        "Browserbase": "browserbase",
        "Supermemory": "supermemory",
        "Metronome": "metronome",
        "Inngest": "inngest",
        "Daytona": "daytona",
        "Runloop": "runloop",
        "WorkOS": "workos",
        "Auth0": "auth0",
        "Privy": "privy",
        "Neon": "neon",
        "Supabase": "supabase",
        "Turso": "turso",
        "Prisma": "prisma",
        "Upstash": "upstash",
        "Chroma": "chroma",
        "Sentry": "sentry",
        "PostHog": "posthog",
        "Amplitude": "amplitude",
        "Mixpanel": "mixpanel",
        "Algolia": "algolia",
        "Exa": "exa",
        "Parallel": "parallel",
        "E2B": "e2b",
        "Blaxel": "blaxel",
        "Railway": "railway",
        "GitLab": "gitlab",
        "Wix": "wix",
    }
    if provider_name in special:
        return special[provider_name]
    return provider_name.lower().replace(".", "_").replace("-", "_")


def ref_of(s: dict) -> str:
    return s.get("reference") or f"{s['provider_name'].lower()}/{s['service_id']}"


def catalog_deployables() -> list[str]:
    refs = []
    for s in CATALOG["data"]["services"]:
        if s.get("kind") != "deployable":
            continue
        refs.append(ref_of(s))
    return sorted(refs)


def declared_rust_files(root: Path) -> list[Path]:
    """`.rs` files reachable from `root/mod.rs` via `mod` / `pub mod` decls.

    Held integrations keep source on disk but omit the parent `pub mod` in
    `providers/mod.rs` (or a nested `mod` for a single held sibling). Those
    files are ignored here so EXCL ownership stays accurate.
    """
    root_mod = root / "mod.rs"
    if not root_mod.is_file():
        return sorted(root.rglob("*.rs"))

    out: list[Path] = []
    seen: set[Path] = set()

    def visit(mod_file: Path, dir_path: Path) -> None:
        if mod_file in seen:
            return
        seen.add(mod_file)
        out.append(mod_file)
        text = mod_file.read_text()
        for name in MOD_RE.findall(text):
            as_file = dir_path / f"{name}.rs"
            as_dir = dir_path / name
            nested = as_dir / "mod.rs"
            if nested.is_file():
                visit(nested, as_dir)
            elif as_file.is_file():
                if as_file not in seen:
                    seen.add(as_file)
                    out.append(as_file)
                    # Nested `mod` decls inside a single-file module.
                    for nested_name in MOD_RE.findall(as_file.read_text()):
                        nested_file = dir_path / f"{nested_name}.rs"
                        nested_mod = dir_path / nested_name / "mod.rs"
                        if nested_mod.is_file():
                            visit(nested_mod, dir_path / nested_name)
                        elif nested_file.is_file() and nested_file not in seen:
                            seen.add(nested_file)
                            out.append(nested_file)

    visit(root_mod, root)
    return sorted(out)


def integration_references() -> set[str]:
    found: set[str] = set()
    for path in declared_rust_files(PROVIDERS):
        found.update(REFERENCE_RE.findall(path.read_text()))
    return found


def check_orphans() -> int:
    """Fail if any catalog deployable is unowned (not registered / substrate / EXCL)."""
    registered = integration_references()
    owned = registered | set(EXCL) | set(SUBSTRATE_ONLY_HOSTING)
    deployables = catalog_deployables()
    orphans = [ref for ref in deployables if ref not in owned]
    conflicts = sorted(ref for ref in EXCL if ref in registered)
    if orphans or conflicts:
        if orphans:
            print(
                "unowned catalog deployables — register an integration, "
                "add a substrate-only hosting ref, or add to EXCL:",
                file=sys.stderr,
            )
            for ref in orphans:
                print(f"  {ref}", file=sys.stderr)
        if conflicts:
            print(
                "EXCL entries that are also registered integrations "
                "(remove from EXCL or the registry):",
                file=sys.stderr,
            )
            for ref in conflicts:
                print(f"  {ref}", file=sys.stderr)
        return 1
    print(
        f"ok: {len(deployables)} deployables owned "
        f"({len(registered)} registered, "
        f"{len(SUBSTRATE_ONLY_HOSTING)} substrate-only, "
        f"{len(EXCL)} excl)"
    )
    return 0


def outputs_for(ref: str):
    return OUTPUT_HINTS.get(ref, [("API_KEY", "api_key", True)])


def rust_type(prop: dict, required: bool) -> str:
    t = prop.get("type")
    if t == "integer":
        return "i64" if required else "Option<i64>"
    if t == "boolean":
        return "bool" if required else "Option<bool>"
    if t == "number":
        return "f64" if required else "Option<f64>"
    return "String" if required else "Option<String>"


def main() -> None:
    services = []
    for s in CATALOG["data"]["services"]:
        if s.get("kind") != "deployable":
            continue
        ref = ref_of(s)
        if ref in IMPLEMENTED or ref in EXCL:
            continue
        services.append(s)

    by_family: dict[str, list] = defaultdict(list)
    for s in services:
        by_family[family_dir(s["provider_name"])].append(s)

    registry_rows: list[tuple[str, str]] = []
    assert_lines: list[str] = []
    top_mods: list[str] = []

    for fam, svcs in sorted(by_family.items()):
        fam_path = PROVIDERS / fam
        fam_path.mkdir(parents=True, exist_ok=True)
        mod_lines = [
            f"//! {svcs[0]['provider_name']} catalog resources via Stripe Projects.",
            "//!",
            "//! Output envelopes are provisional until pinned by `xtask discover`.",
        ]
        if fam == "wordpress_com":
            mod_lines.append(
                "//! Excluded: `wordpress.com/domain` (non-refundable domain purchase)."
            )
        mod_lines.append("")

        for s in sorted(svcs, key=lambda x: x["service_id"]):
            ref = ref_of(s)
            sid = s["service_id"].replace(":", "_").replace("-", "_")
            type_base = camel(s["provider_name"].replace(".", " ")) + camel(s["service_id"])
            config_type = f"{type_base}Config"
            provider_key = SHORT_PROVIDER.get(
                ref,
                f"{ref.split('/')[0].replace('.', '-')}-{s['service_id'].replace(':', '-').replace('_', '-')}",
            )
            resource_kind = f"integration-{provider_key}"
            provider_prefix = (
                ref.split("/")[0].upper().replace(".", "_").replace("-", "_")
            )
            if provider_prefix == "BASE44_PROJECTS":
                provider_prefix = "BASE44"

            schema = s.get("configuration_schema") or {}
            props = schema.get("properties") or {}
            required = set(schema.get("required") or [])
            prop_keys = sorted(props.keys(), key=lambda k: (0 if k in required else 1, k))

            fields_struct: list[str] = []
            build_fields: list[str] = []
            validate_lines: list[str] = []
            sample_fields: list[str] = []
            toml_lines = [f'provider = "{provider_key}"']

            RUST_KEYWORDS = {
                "type",
                "ref",
                "self",
                "crate",
                "super",
                "as",
                "fn",
                "let",
                "mut",
                "pub",
                "mod",
                "use",
                "impl",
                "trait",
                "where",
                "async",
                "await",
                "move",
                "box",
                "match",
                "if",
                "else",
                "loop",
                "while",
                "for",
                "in",
                "break",
                "continue",
                "return",
                "yield",
                "dyn",
                "true",
                "false",
                "struct",
                "enum",
                "const",
                "static",
                "unsafe",
                "extern",
                "id",
            }

            for key in prop_keys:
                prop = props[key]
                req = key in required
                ty = prop.get("type")
                rust_ty = rust_type(prop, req)
                rust_key = f"r#{key}" if key in RUST_KEYWORDS else key
                attrs = ""
                if key in RUST_KEYWORDS:
                    attrs += f'    #[serde(rename = "{key}")]\n'
                if rust_ty.startswith("Option"):
                    attrs += '    #[serde(skip_serializing_if = "Option::is_none")]\n'
                fields_struct.append(f"{attrs}    pub {rust_key}: {rust_ty},")

                if ty == "integer":
                    if req:
                        build_fields.append(
                            f'            {rust_key}: super::int_required(ctx, &config, "{key}")?,'
                        )
                        validate_lines.append(
                            f'    if config.get("{key}").and_then(toml::Value::as_integer).is_none() {{\n'
                            f"        return Err(IntegrationError::ConfigInvalid {{\n"
                            f'            location: format!("integrations.{{name}}.{key}"),\n'
                            f'            detail: "{key} is required and must be an integer".into(),\n'
                            f"        }});\n"
                            f"    }}"
                        )
                        default = prop.get("default")
                        sample_fields.append(
                            f"                {rust_key}: {default if default is not None else 1},"
                        )
                        toml_lines.append(
                            f"{key} = {default if default is not None else 1}"
                        )
                    else:
                        build_fields.append(
                            f'            {rust_key}: super::int_optional(ctx, &config, "{key}")?,'
                        )
                        sample_fields.append(f"                {rust_key}: None,")
                elif ty == "boolean":
                    if req:
                        build_fields.append(
                            f'            {rust_key}: super::bool_required(ctx, &config, "{key}")?,'
                        )
                        validate_lines.append(
                            f'    if config.get("{key}").and_then(toml::Value::as_bool).is_none() {{\n'
                            f"        return Err(IntegrationError::ConfigInvalid {{\n"
                            f'            location: format!("integrations.{{name}}.{key}"),\n'
                            f'            detail: "{key} is required and must be a boolean".into(),\n'
                            f"        }});\n"
                            f"    }}"
                        )
                        sample_fields.append(f"                {rust_key}: false,")
                        toml_lines.append(f"{key} = false")
                    else:
                        build_fields.append(
                            f'            {rust_key}: super::bool_optional(ctx, &config, "{key}")?,'
                        )
                        sample_fields.append(f"                {rust_key}: None,")
                elif ty == "number":
                    # toml floats; accept integer literals too.
                    if req:
                        build_fields.append(
                            f'            {rust_key}: config.get("{key}")'
                            f".and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))"
                            f".ok_or_else(|| IntegrationError::ConfigInvalid {{"
                            f'                location: format!("integrations.{{}}.{key}", ctx.logical_name),'
                            f'                detail: "{key} is required and must be a number".into(),'
                            f"            }})?,"
                        )
                        validate_lines.append(
                            f'    if config.get("{key}").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).is_none() {{\n'
                            f"        return Err(IntegrationError::ConfigInvalid {{\n"
                            f'            location: format!("integrations.{{name}}.{key}"),\n'
                            f'            detail: "{key} is required and must be a number".into(),\n'
                            f"        }});\n"
                            f"    }}"
                        )
                        default = prop.get("default")
                        sample_fields.append(
                            f"                {rust_key}: {float(default) if default is not None else 1.0},"
                        )
                        toml_lines.append(
                            f"{key} = {float(default) if default is not None else 1.0}"
                        )
                    else:
                        build_fields.append(
                            f'            {rust_key}: match config.get("{key}") {{'
                            f" None => None,"
                            f" Some(v) => Some(v.as_float().or_else(|| v.as_integer().map(|i| i as f64))"
                            f".ok_or_else(|| IntegrationError::ConfigInvalid {{"
                            f'                location: format!("integrations.{{}}.{key}", ctx.logical_name),'
                            f'                detail: "{key} must be a number when set".into(),'
                            f"            }})?),"
                            f" }},"
                        )
                        sample_fields.append(f"                {rust_key}: None,")
                else:
                    if req:
                        build_fields.append(
                            f'            {rust_key}: super::interp_required(ctx, &config, "{key}")?,'
                        )
                        validate_lines.append(
                            f'    registry::config_string(config, "{key}").map_err(|err| IntegrationError::ConfigInvalid {{\n'
                            f'        location: format!("integrations.{{name}}.{key}"),\n'
                            f"        detail: err.to_string(),\n"
                            f"    }})?;"
                        )
                        sample = prop.get("enum")[0] if prop.get("enum") else f"test-{key}"
                        sample_fields.append(
                            f'                {rust_key}: "{sample}".into(),'
                        )
                        toml_lines.append(f'{key} = "{sample}"')
                    else:
                        build_fields.append(
                            f'            {rust_key}: super::interp_optional(ctx, &config, "{key}")?,'
                        )
                        sample_fields.append(f"                {rust_key}: None,")

            out_fields = outputs_for(ref)
            outputs_list = ", ".join(f'"{name}"' for _, name, _ in out_fields)
            output_fields_rs = ",\n        ".join(
                f'("{suf}", "{name}", {"true" if req else "false"})'
                for suf, name, req in out_fields
            )
            first_out = out_fields[0][1]
            env_json = {
                f"{provider_prefix}_{suf}": f"val_{name}" for suf, name, _ in out_fields
            }

            stub_schema = schema if schema else {
                "type": "object",
                "required": [],
                "additionalProperties": False,
                "properties": {},
            }
            pricing_type = (s.get("pricing") or {}).get("type") or "component"
            catalog_envelope = json.dumps(
                {
                    "ok": True,
                    "command": "projects catalog",
                    "data": {
                        "last_updated": "2026-07-11T00:00:00Z",
                        "services": [
                            {
                                "id": f"prvsvc_{sid}",
                                "object": "v2.provisioning.provider_service_detail",
                                "provider_id": f"prvdr_{fam}",
                                "provider_name": s["provider_name"],
                                "service_id": s["service_id"],
                                "categories": ["database"],
                                "kind": "deployable",
                                "scope": "project",
                                "availability": "available",
                                "development": False,
                                "livemode": True,
                                "pricing": {"type": pricing_type},
                                "configuration_schema": stub_schema,
                            }
                        ],
                    },
                },
                separators=(",", ":"),
            )

            if fields_struct:
                config_struct = (
                    f"#[derive(Debug, Serialize)]\npub struct {config_type} {{\n"
                    + "\n".join(fields_struct)
                    + "\n}"
                )
                build_fn = (
                    f"    fn build_config(ctx: &ProvisionContext<'_>) -> Result<{config_type}, IntegrationError> {{\n"
                    f"        let config = super::integration_config(ctx)?;\n"
                    f"        Ok({config_type} {{\n"
                    + "\n".join(build_fields)
                    + f"\n        }})\n    }}"
                )
                sample_config = (
                    f"{config_type} {{\n" + "\n".join(sample_fields) + "\n            }"
                )
            else:
                config_struct = f"#[derive(Debug, Serialize)]\npub struct {config_type} {{}}"
                build_fn = (
                    f"    fn build_config(ctx: &ProvisionContext<'_>) -> Result<{config_type}, IntegrationError> {{\n"
                    f"        let _ = super::integration_config(ctx)?;\n"
                    f"        Ok({config_type} {{}})\n"
                    f"    }}"
                )
                sample_config = f"{config_type} {{}}"

            validate_body = "\n".join(validate_lines)
            toml_block = "\n".join(toml_lines)
            env_block = json.dumps(env_json)

            rs = f'''//! `{ref}` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{{ConfigScope, Hostable, IntegrationHosting}};
use crate::registry;

pub const RESOURCE_KIND: &str = "{resource_kind}";

{config_struct}

impl CatalogService for {config_type} {{
    const REFERENCE: &'static str = "{ref}";
}}

#[derive(Debug)]
pub struct {type_base};

impl Hostable for {type_base} {{
    const PROVIDER: &'static str = "{provider_key}";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[{outputs_list}];
}}

impl FamilyResource for {type_base} {{
    type Config = {config_type};
    const PROVIDER_PREFIX: &'static str = "{provider_prefix}";
    // Provisional until pinned by `mise run discover {ref}`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        {output_fields_rs}
    ];

{build_fn}
}}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {{
{validate_body}
    let _ = (name, config);
    Ok(())
}}

#[cfg(test)]
mod tests {{
    use super::*;
    use crate::ProviderOps;
    use crate::resource::ResourcePayload;
    use stackless_core::def::StackDef;
    use stackless_stripe_projects::stripe::StripeProjects;
    use stackless_stripe_projects::test_support;

    #[test]
    fn config_matches_catalog() {{
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &{sample_config},
        );
        assert!(
            failures.is_empty(),
            "{ref} catalog gaps:\\n{{}}",
            failures.join("\\n")
        );
    }}

    const CATALOG_ENVELOPE: &str = r##"{catalog_envelope}"##;

    fn test_def() -> StackDef {{
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
{toml_block}
[services.api]
source = {{ repo = "r", ref = "main" }}
env = {{ OUT = "${{integrations.res.{first_out}}}" }}
health = {{ path = "/health" }}
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }}

    #[tokio::test]
    async fn provision_records_outputs() {{
        let runner = test_support::provision_script(
            CATALOG_ENVELOPE,
            serde_json::json!({env_block}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\\nname=\\"atto\\"\\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = {type_base}
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "res",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "{resource_kind}");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["{first_out}"], "val_{first_out}");
    }}
}}
'''
            out_path = fam_path / f"{sid}.rs"
            out_path.write_text(rs)
            mod_lines.append(f"pub mod {sid};")
            registry_rows.append((f"{fam}::{sid}", type_base))
            assert_lines.append(
                f"        assert_outputs_match::<{fam}::{sid}::{type_base}>();"
            )

        mod_lines.extend(
            [
                "",
                "pub(crate) use crate::resource::{",
                "    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional,",
                "    int_required, integration_config, interp_optional, interp_required,",
                "};",
                "",
            ]
        )
        (fam_path / "mod.rs").write_text("\n".join(mod_lines))
        top_mods.append(fam)

    # Do not rewrite providers/mod.rs or registry.rs — those are maintained
    # per-PR (union-sort on land). Rewriting here drops landed providers.
    _ = (assert_lines, registry_rows, top_mods)
    print(f"generated {len(services)} services across {len(by_family)} families under {PROVIDERS}")
    print("skipping providers/mod.rs and registry.rs rewrites (maintain by hand)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check-orphans",
        action="store_true",
        help=(
            "verify every catalog deployable is a registered integration, "
            "substrate-only hosting ref, or EXCL entry"
        ),
    )
    args = parser.parse_args()
    if args.check_orphans:
        raise SystemExit(check_orphans())
    main()
