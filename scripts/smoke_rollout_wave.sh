#!/usr/bin/env bash
# Roll out live smokes for a provider wave: discover credential envelopes, then smoke.
#
# Usage:
#   smoke_rollout_wave.sh 1    # wave 1 vendors from docs/PROVIDER-WAVES.md
#   smoke_rollout_wave.sh neon # single vendor
#
# Requires STRIPE_API_KEY, stripe CLI + projects plugin, and linked vendors.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/fixtures/smoke/integrations/manifest.json"

[ -f "$ROOT/.stackless.env" ] && { set -a; . "$ROOT/.stackless.env"; set +a; }

if [ -z "${STRIPE_API_KEY:-}" ]; then
  echo "::error::STRIPE_API_KEY required — set in .stackless.env or environment" >&2
  exit 1
fi

WAVE="${1:-}"
if [ -z "$WAVE" ]; then
  echo "usage: $0 <wave-number|vendor>" >&2
  exit 2
fi

declare -A WAVES
WAVES[1]="neon supabase turso upstash auth0 workos privy prisma"
WAVES[2]="planetscale clickhouse chroma sentry posthog amplitude mixpanel algolia"
WAVES[3]="openrouter exa firecrawl parallel elevenlabs heygen huggingface inngest"
WAVES[4]="e2b daytona browserbase blaxel runloop kernel agentmail agentphone"
WAVES[5]="railway gitlab laravel_cloud wordpress.com base44_projects wix postalform metronome supermemory"
WAVES[6]="render flyio"

if [ -n "${WAVES[$WAVE]+x}" ]; then
  VENDORS="${WAVES[$WAVE]}"
else
  VENDORS="$WAVE"
fi

echo "rollout vendors: $VENDORS"

for vendor in $VENDORS; do
  echo "=== vendor $vendor ==="
  slugs=$(python3 - - "$MANIFEST" "$vendor" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
vendor = sys.argv[2]
for e in manifest["integrations"]:
    if e["vendor"] == vendor:
        print(e["slug"])
PY
)
  for slug in $slugs; do
    dir="$ROOT/fixtures/smoke/integrations/$slug"
    ref=$(python3 - - "$MANIFEST" "$slug" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
slug = sys.argv[2]
for e in manifest["integrations"]:
    if e["slug"] == slug:
        print(e["reference"])
        break
PY
)
    echo "--- discover $ref ($slug) ---"
    if [ ! -d "$dir/.projects" ]; then
      (cd "$dir" && stripe projects init "smoke-$slug" --skip-skills --accept-tos --yes) || true
    fi
    mise run discover "$ref" -- --dir "$dir" || echo "discover failed for $ref"
    echo "--- smoke $slug ---"
    mise run "smoke-integration-$slug" || echo "smoke failed for $slug"
  done
done
