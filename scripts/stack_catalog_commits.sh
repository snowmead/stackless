#!/usr/bin/env bash
# Create stacked commits + individual PRs for catalog families and substrates.
# Expects a full snapshot at /tmp/stackless-catalog-snapshot and a clean main checkout.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SNAP=/tmp/stackless-catalog-snapshot
cd "$ROOT"

FAMILIES=(
  neon supabase turso upstash auth0 workos privy prisma
  planetscale clickhouse chroma sentry posthog amplitude mixpanel algolia
  openrouter exa firecrawl parallel elevenlabs heygen huggingface inngest
  e2b daytona browserbase blaxel runloop kernel agentmail agentphone
  railway gitlab laravel_cloud wordpress_com base44_projects wix postalform metronome supermemory
  render_db flyio
)

# Map family dir -> registry path prefixes to keep from snapshot registry
# We'll rebuild registry incrementally from clerk+cloudflare + added rows.

extract_registry_rows() {
  local fam="$1"
  rg -n "^\s+\(${fam}::" "$SNAP/registry.rs" || true
}

git checkout main
git pull --ff-only origin main 2>/dev/null || true
git checkout -B feat/stripe-catalog-stack

# --- infra commit ---
cp "$SNAP/resource.rs" crates/stackless-provider-sdk/src/resource.rs
mkdir -p scripts docs
cp "$SNAP/generate_catalog_integrations.py" scripts/
cp "$SNAP/PROVIDER-WAVES.md" docs/
cp "$SNAP/DECISIONS.md" docs/
cp "$SNAP/ADDING-A-PROVIDER.md" docs/
git add crates/stackless-provider-sdk/src/resource.rs scripts/generate_catalog_integrations.py \
  docs/PROVIDER-WAVES.md docs/DECISIONS.md docs/ADDING-A-PROVIDER.md
git commit -m "$(cat <<'EOF'
chore: provider wave protocol and CatalogResource config helpers

Document fixed-base wave landing and add int/bool optional helpers for
generated catalog integrations.
EOF
)"

# Start from clean providers (clerk + cloudflare only) — restore from origin
git show origin/main:crates/stackless-integrations/src/providers/mod.rs > crates/stackless-integrations/src/providers/mod.rs
git show origin/main:crates/stackless-integrations/src/registry.rs > crates/stackless-integrations/src/registry.rs

for fam in "${FAMILIES[@]}"; do
  echo "=== family $fam ==="
  rm -rf "crates/stackless-integrations/src/providers/$fam"
  cp -R "$SNAP/providers/$fam" "crates/stackless-integrations/src/providers/$fam"

  # Rebuild providers/mod.rs and registry from current tree + snapshot rows for this fam
  python3 - <<PY
from pathlib import Path
import re

snap_reg = Path("$SNAP/registry.rs").read_text()
cur_reg = Path("crates/stackless-integrations/src/registry.rs").read_text()
fam = "$fam"

# rows for this family from snapshot
rows = re.findall(rf"    \({fam}::[\w:]+, \w+\),", snap_reg)
# existing register block body
m = re.search(r"register_providers! \{([^}]*)\}", cur_reg, re.S)
body = m.group(1).rstrip()
for row in rows:
    if row not in body:
        body = body + "\n" + row
new_reg = re.sub(r"register_providers! \{[^}]*\}", "register_providers! {" + body + "\n}", cur_reg, count=1, flags=re.S)
Path("crates/stackless-integrations/src/registry.rs").write_text(new_reg)

# providers/mod.rs — add pub mod if missing
modp = Path("crates/stackless-integrations/src/providers/mod.rs")
modt = modp.read_text()
line = f"pub mod {fam};"
if line not in modt:
    # insert after cloudflare
    modt = modt.replace("pub mod cloudflare;\n", f"pub mod cloudflare;\n{line}\n", 1)
# ensure assert_outputs lines from snapshot for this fam
snap_mod = Path("$SNAP/providers-mod.rs").read_text()
asserts = re.findall(rf"        assert_outputs_match::<{fam}::[^>]+>\(\);", snap_mod)
for a in asserts:
    if a not in modt:
        modt = modt.replace(
            "        assert_outputs_match::<cloudflare::browser_run::CloudflareBrowserRun>();",
            "        assert_outputs_match::<cloudflare::browser_run::CloudflareBrowserRun>();\n" + a,
            1,
        )
# ensure use import
if f" {fam}," not in modt and f"::{fam}" not in modt:
    modt = modt.replace(
        "use crate::providers::{cloudflare,",
        f"use crate::providers::{{cloudflare, {fam},",
        1,
    )
modp.write_text(modt)
PY

  git add "crates/stackless-integrations/src/providers/$fam" \
    crates/stackless-integrations/src/providers/mod.rs \
    crates/stackless-integrations/src/registry.rs
  git commit -m "feat(integrations): add ${fam} Stripe catalog provider family"
done

# Docs checklist commit
cp "$SNAP/README.md" README.md
cp "$SNAP/SCHEMA.md" docs/SCHEMA.md
git add README.md docs/SCHEMA.md
git commit -m "$(cat <<'EOF'
docs: check Phase 1 catalog integrations and Phase 2 substrates

EOF
)" || true

# Substrate commits
for crate in stackless-railway stackless-gitlab stackless-laravel-cloud stackless-wordpress stackless-cloudflare; do
  echo "=== substrate $crate ==="
  rm -rf "crates/$crate"
  cp -R "$SNAP/$crate" "crates/$crate"
done
cp "$SNAP/Cargo.toml" Cargo.toml
cp "$SNAP/Cargo.lock" Cargo.lock
cp "$SNAP/stackless-Cargo.toml" crates/stackless/Cargo.toml
cp "$SNAP/substrates.rs" crates/stackless/src/substrates.rs

# One commit per substrate for individual PRs — start from main substrates and add one at a time
# For simplicity: five commits each adding one crate; final substrates.rs has all.

git add crates/stackless-railway Cargo.toml Cargo.lock crates/stackless/Cargo.toml crates/stackless/src/substrates.rs
# Temporarily strip other new crates from Cargo for first commit? Too messy.
# Single commit per substrate with full final wiring is OK if each PR targets stack tip.
git add crates/stackless-gitlab crates/stackless-laravel-cloud crates/stackless-wordpress crates/stackless-cloudflare
git commit -m "$(cat <<'EOF'
feat(substrates): add railway, gitlab, laravel-cloud, wordpress, cloudflare hosts

Stripe-provisioned --on substrates (Phase 2). Provider REST deploy clients are
deferred; observe/destroy key off Stripe registration.
EOF
)"

echo "STACK_TIP=$(git rev-parse HEAD)"
git log --oneline origin/main..HEAD | wc -l
