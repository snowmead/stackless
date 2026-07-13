#!/usr/bin/env python3
"""Generate live-smoke fixtures for every catalog integration in the registry.

Writes:
  fixtures/smoke/integrations/manifest.json
  fixtures/smoke/integrations/<slug>/stackless.toml
  fixtures/smoke/integrations/<slug>/.gitignore
  fixtures/smoke/integrations/mise-tasks.toml  (include in mise.toml)

Regenerate: python3 scripts/generate_smoke_fixtures.py
"""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROVIDERS = ROOT / "crates/stackless-integrations/src/providers"
INTEGRATIONS = ROOT / "fixtures/smoke/integrations"
CATALOG_PATH = (
    ROOT / "crates/stackless-stripe-projects/tests/fixtures/catalog.json"
)
MISE = ROOT / "mise.toml"

GITIGNORE = """# Volatile Stripe Projects runtime state (not part of the fixture).
.projects/cache
.projects/vault
.projects/state.test.json
.projects/state.local.test.json
.env
.env.*
!.env.example
"""

PROBE_RUN = (
    'run = "sh -c \'d=$(mktemp -d); printf stackless-smoke-ok > \\"$d/index.html\\"; '
    'exec python3 -m http.server $PORT --bind 127.0.0.1 --directory \\"$d\\"\'"'
)

# Stripe `projects link <vendor>` slug (lowercased catalog provider_name).
VENDOR_LINK = {
    "WordPress.com": "wordpress.com",
    "Laravel_Cloud": "laravel_cloud",
    "Base44_Projects": "base44_projects",
}

INTEGRATION_NAME = {
    "auth": "auth",
    "postgres": "db",
    "mysql": "db",
    "postgresql": "db",
    "mongo": "db",
    "database": "db",
    "kv": "cache",
    "r2:bucket": "bucket",
    "bucket": "bucket",
    "redis": "cache",
    "valkey": "cache",
    "qstash": "queue",
    "vector": "vector",
    "search": "search",
    "analytics": "analytics",
    "sandbox": "sandbox",
    "project": "project",
    "application": "app",
    "app": "app",
    "api": "api",
    "tts": "tts",
    "mail": "mail",
    "memory": "memory",
    "site": "site",
    "headless": "headless",
    "hosting": "hosting",
    "mpg": "db",
    "sprite": "sprite",
    "seer": "seer",
    "platform": "platform",
    "client": "auth",
    "number": "phone",
    "agent-drive": "drive",
    "browser-run": "browser",
    "workers": "workers",
    "workers-ai": "workers-ai",
    "queues": "queues",
    "hyperdrive": "hyperdrive",
    "d1": "db",
}


def vendor_link(provider_name: str) -> str:
    return VENDOR_LINK.get(provider_name, provider_name.lower())


def slug_for(provider: str, reference: str) -> str:
    """Fixture directory slug, e.g. cloudflare-kv."""
    return provider


def integration_key(service_id: str) -> str:
    for part in reversed(service_id.replace(":", "-").split("-")):
        if part in INTEGRATION_NAME:
            return INTEGRATION_NAME[part]
    base = service_id.split(":")[0].split("-")[0]
    return INTEGRATION_NAME.get(base, "resource")


def parse_outputs(text: str, cloudflare_mod: str) -> list[str]:
    if "WORKERS_FAMILY_OUTPUTS" in text:
        m = re.search(
            r"WORKERS_FAMILY_OUTPUTS: &\[&str\] = &\[(.*?)\];",
            cloudflare_mod,
            re.S,
        )
        if m:
            return re.findall(r'"([^"]+)"', m.group(1))
    m = re.search(r"const OUTPUTS: &'static \[&'static str\] = &\[(.*?)\];", text, re.S)
    if not m:
        return ["api_key"]
    return re.findall(r'"([^"]+)"', m.group(1))


def default_for_field(name: str, prop: dict, schema: dict) -> str:
    allowed = prop.get("enum") or prop.get("allowed") or []
    if allowed:
        val = allowed[0]
        if isinstance(val, str):
            return val
    if name in ("name", "title", "app_name", "cluster"):
        if name == "cluster":
            return "smoke"
        return "stackless-smoke-${instance.name}"
    if name == "region":
        return "us-east"
    if name == "credential_set":
        return "development"
    if name == "version":
        return "16"
    if name == "disk_size":
        return "15"
    if prop.get("type") == "boolean":
        return "false"
    if prop.get("type") == "integer":
        return "1"
    return "stackless-smoke"


def minimal_config(schema: dict | None) -> dict[str, str]:
    if not schema:
        return {}
    props = schema.get("properties") or {}
    required = schema.get("required") or []
    out: dict[str, str] = {}
    for key in required:
        prop = props.get(key) or {}
        out[key] = default_for_field(key, prop, schema)
    return out


def collect_entries() -> list[dict]:
    catalog = json.loads(CATALOG_PATH.read_text())
    by_ref = {}
    for s in catalog["data"]["services"]:
        ref = s.get("reference") or f"{s['provider_name'].lower()}/{s['service_id']}"
        by_ref[ref] = s

    cf_mod = (PROVIDERS / "cloudflare/mod.rs").read_text()
    entries: list[dict] = []

    # Clerk is not under a subdir pattern with REFERENCE in same file layout.
    clerk_path = PROVIDERS / "clerk.rs"
    if clerk_path.exists():
        t = clerk_path.read_text()
        ref = "clerk/auth"
        service = by_ref[ref]
        outputs = ["secret_key", "publishable_key"]
        provider = "clerk"
        slug = slug_for(provider, ref)
        entries.append(
            {
                "reference": ref,
                "slug": slug,
                "provider": provider,
                "vendor": vendor_link(service["provider_name"]),
                "paid": service["pricing"]["type"] == "paid",
                "integration_name": "auth",
                "config": {
                    "app_name": "stackless-smoke-${instance.name}",
                    "credential_set": "development",
                },
                "outputs": outputs,
                "stack_name": f"smoke-{slug}",
            }
        )

    for path in sorted(PROVIDERS.rglob("*.rs")):
        if path.name in ("mod.rs", "clerk.rs"):
            continue
        text = path.read_text()
        ref_m = re.search(r'const REFERENCE: &\'static str = "([^"]+)"', text)
        prov_m = re.search(r'const PROVIDER: &\'static str = "([^"]+)"', text)
        if not ref_m or not prov_m:
            continue
        ref = ref_m.group(1)
        provider = prov_m.group(1)
        service = by_ref.get(ref)
        if not service:
            continue
        outputs = parse_outputs(text, cf_mod)
        svc_id = service["service_id"]
        slug = slug_for(provider, ref)
        entries.append(
            {
                "reference": ref,
                "slug": slug,
                "provider": provider,
                "vendor": vendor_link(service["provider_name"]),
                "paid": service["pricing"]["type"] == "paid",
                "integration_name": integration_key(svc_id),
                "config": minimal_config(service.get("configuration_schema")),
                "outputs": outputs,
                "stack_name": f"smoke-{slug}",
            }
        )

    entries.sort(key=lambda e: e["slug"])
    return entries


def render_toml(entry: dict) -> str:
    lines = [
        f"# Live smoke: provisions `{entry['reference']}` through Stripe Projects",
        f"# under `--on local`. Run via:",
        f"#   mise run smoke-integration-{entry['slug']}",
        f"# One-time: `stripe projects link {entry['vendor']}` (account-level).",
        "",
        "[stack]",
        f'name = "{entry["stack_name"]}"',
        "",
        "[stack.projects]",
        "[stack.projects.stripe]",
        "# project anchor recorded on first successful `up`",
        "",
        f"[integrations.{entry['integration_name']}]",
        f'provider = "{entry["provider"]}"',
    ]
    for key, val in entry["config"].items():
        if "${" in val:
            lines.append(f'{key} = "{val}"')
        else:
            lines.append(f'{key} = "{val}"')
    lines.extend(
        [
            "",
            "[services.web]",
            'source = { repo = "https://github.com/snowmead/stackless", ref = "main" }',
            "root_origin = true",
            'health = { path = "/", contains = "stackless-smoke-ok" }',
        ]
    )
    probe_out = entry["outputs"][0]
    integ = entry["integration_name"]
    lines.append(
        f'env = {{ PROBE = "${{integrations.{integ}.{probe_out}}}" }}'
    )
    lines.extend(["", "[services.web.local]", PROBE_RUN, ""])
    return "\n".join(lines)


def render_manifest(entries: list[dict]) -> dict:
    manifest_entries = []
    for e in entries:
        probe_out = e["outputs"][0]
        manifest_entries.append(
            {
                "reference": e["reference"],
                "slug": e["slug"],
                "provider": e["provider"],
                "vendor": e["vendor"],
                "paid": e["paid"],
                "integration_name": e["integration_name"],
                "config": e["config"],
                "probe_env": {
                    "PROBE": f"${{integrations.{e['integration_name']}.{probe_out}}}"
                },
                "fixture": f"fixtures/smoke/integrations/{e['slug']}/stackless.toml",
                "prefix": f"smoke-i-{e['slug']}",
            }
        )
    vendors = sorted({e["vendor"] for e in entries})
    return {
        "version": 1,
        "count": len(manifest_entries),
        "vendors": vendors,
        "integrations": manifest_entries,
    }


def render_mise_tasks(entries: list[dict]) -> str:
    lines = [
        "# BEGIN smoke-integration tasks (generated by scripts/generate_smoke_fixtures.py)",
    ]
    task_lines: list[tuple[str, str]] = []
    for e in entries:
        slug = e["slug"]
        fixture = f"fixtures/smoke/integrations/{slug}/stackless.toml"
        prefix = f"smoke-i-{slug}"
        paid = " --confirm-paid" if e["paid"] else ""
        key = f"smoke-integration-{slug}"
        val = f'bash fixtures/smoke/run.sh local {fixture} {prefix}{paid}'
        task_lines.append((key, val))
    width = max(len(k) for k, _ in task_lines)
    for key, val in task_lines:
        lines.append(f'{key.ljust(width)} = "{val}"')
    lines.extend(
        [
            "smoke-integrations".ljust(width)
            + ' = "bash fixtures/smoke/run-integrations.sh --from-manifest"',
            "smoke-integrations-vendor".ljust(width)
            + ' = "bash fixtures/smoke/run-integrations.sh"',
            "smoke-integrations-audit".ljust(width)
            + ' = "bash fixtures/smoke/stripe-link-audit.sh"',
            "# END smoke-integration tasks",
        ]
    )
    return "\n".join(lines) + "\n"


def patch_mise(mise_tasks: str) -> None:
    text = MISE.read_text()
    begin = "# BEGIN smoke-integration tasks"
    end = "# END smoke-integration tasks"
    if begin in text:
        pre, rest = text.split(begin, 1)
        _, post = rest.split(end, 1)
        # drop through end line inclusive from old block
        post_lines = post.split("\n", 1)
        post = post_lines[1] if len(post_lines) > 1 else ""
        new_text = pre.rstrip() + "\n\n" + mise_tasks.rstrip() + "\n" + post.lstrip("\n")
    else:
        marker = "smoke-fleet = '''"
        if marker not in text:
            raise SystemExit("mise.toml: cannot find insertion point")
        pre, post = text.split(marker, 1)
        new_text = pre.rstrip() + "\n\n" + mise_tasks.rstrip() + "\n\n" + marker + post
    MISE.write_text(new_text)


def main() -> None:
    entries = collect_entries()
    if len(entries) != 67:
        raise SystemExit(f"expected 67 integrations, got {len(entries)}")

    INTEGRATIONS.mkdir(parents=True, exist_ok=True)
    manifest = render_manifest(entries)
    (INTEGRATIONS / "manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )

    for entry in entries:
        slug_dir = INTEGRATIONS / entry["slug"]
        slug_dir.mkdir(parents=True, exist_ok=True)
        (slug_dir / "stackless.toml").write_text(render_toml(entry))
        (slug_dir / ".gitignore").write_text(GITIGNORE)

    mise_fragment = render_mise_tasks(entries)
    (INTEGRATIONS / "mise-tasks.toml").write_text(mise_fragment)
    patch_mise(mise_fragment)

    import subprocess

    subprocess.run(
        ["taplo", "fmt", str(INTEGRATIONS), str(MISE)],
        check=False,
    )

    print(f"generated {len(entries)} integration smoke fixtures")
    print(f"vendors: {len(manifest['vendors'])}")


if __name__ == "__main__":
    main()
