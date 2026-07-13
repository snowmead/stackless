#!/usr/bin/env bash
# Run live integration smokes from fixtures/smoke/integrations/manifest.json.
#
# Usage:
#   run-integrations.sh --from-manifest              # all vendors, serial per vendor
#   run-integrations.sh --vendor cloudflare          # one vendor's integrations
#   run-integrations.sh --slug neon                  # one integration
#   run-integrations.sh --from-manifest --spacing 900  # custom cloudflare spacing (sec)
#
# Honors STACKLESS_LIVE_SMOKE_REQUIRED=1 (fail on missing STRIPE_API_KEY or unlinked vendor).
set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MANIFEST="$ROOT/fixtures/smoke/integrations/manifest.json"
RUN_ONE="$ROOT/fixtures/smoke/run.sh"

FROM_MANIFEST=0
VENDOR=""
SLUG=""
SPACING=""

while [ $# -gt 0 ]; do
  case "$1" in
    --from-manifest) FROM_MANIFEST=1 ;;
    --vendor) shift; VENDOR="${1:-}" ;;
    --slug) shift; SLUG="${1:-}" ;;
    --spacing) shift; SPACING="${1:-}" ;;
    -h|--help)
      sed -n '1,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ ! -f "$MANIFEST" ]; then
  echo "::error::missing $MANIFEST — run python3 scripts/generate_smoke_fixtures.py" >&2
  exit 1
fi

[ -f "$ROOT/.stackless.env" ] && { set -a; . "$ROOT/.stackless.env"; set +a; }

if [ "${STACKLESS_LIVE_SMOKE_REQUIRED:-}" = "1" ] && [ -z "${STRIPE_API_KEY:-}" ]; then
  echo "::error::STRIPE_API_KEY is required for live integration smoke" >&2
  exit 1
fi

if [ -z "${STRIPE_API_KEY:-}" ]; then
  echo "warn: STRIPE_API_KEY unset — smokes will likely fail" >&2
fi

linked_vendors() {
  if ! command -v stripe >/dev/null 2>&1; then
    echo ""
    return
  fi
  stripe projects status 2>/dev/null | python3 -c '
import json, sys, re
text = sys.stdin.read().strip()
if not text:
    sys.exit(0)
# status may be plain text or json depending on CLI flags
if text.startswith("{"):
    data = json.loads(text)
    providers = data.get("data", {}).get("providers") or data.get("providers") or []
    for p in providers:
        name = p.get("name") or p.get("provider") or ""
        if name:
            print(name.lower())
else:
    for line in text.splitlines():
        m = re.search(r"[✓✔]\s+([A-Za-z0-9._-]+)", line)
        if m:
            print(m.group(1).lower())
' 2>/dev/null || true
}

vendor_linked() {
  local want="$1"
  local v
  while read -r v; do
    [ "$v" = "$want" ] && return 0
  done < <(linked_vendors)
  return 1
}

default_spacing() {
  case "$1" in
    cloudflare) echo 900 ;;  # ~15 min between provisions
    *) echo 0 ;;
  esac
}

passed=0
failed=0
skipped=0
declare -a FAILURES=()

run_entry() {
  local slug="$1" fixture="$2" prefix="$3" paid="$4" vendor="$5"
  local extra=""
  [ "$paid" = "true" ] && extra="--confirm-paid"

  if ! vendor_linked "$vendor"; then
    if [ "${STACKLESS_LIVE_SMOKE_REQUIRED:-}" = "1" ]; then
      echo "::error::vendor $vendor not linked (stripe projects link $vendor)" >&2
      FAILURES+=("$slug:unlinked:$vendor")
      failed=$((failed + 1))
      return 1
    fi
    echo "skip $slug: vendor $vendor not linked"
    skipped=$((skipped + 1))
    return 0
  fi

  echo "=== smoke-integration-$slug ==="
  # shellcheck disable=SC2086
  if bash "$RUN_ONE" local "$fixture" "$prefix" $extra; then
    passed=$((passed + 1))
    return 0
  fi
  FAILURES+=("$slug")
  failed=$((failed + 1))
  return 1
}

read_entries() {
  python3 - "$MANIFEST" "$VENDOR" "$SLUG" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
vendor_filter = sys.argv[2]
slug_filter = sys.argv[3]
for e in manifest["integrations"]:
    if vendor_filter and e["vendor"] != vendor_filter:
        continue
    if slug_filter and e["slug"] != slug_filter:
        continue
    print("\t".join([
        e["slug"],
        e["fixture"],
        e["prefix"],
        "true" if e["paid"] else "false",
        e["vendor"],
    ]))
PY
}

if [ "$FROM_MANIFEST" = "0" ] && [ -z "$VENDOR" ] && [ -z "$SLUG" ]; then
  echo "specify --from-manifest, --vendor, or --slug" >&2
  exit 2
fi

last_vendor=""
while IFS=$'\t' read -r slug fixture prefix paid vendor; do
  if [ -n "$last_vendor" ] && [ "$vendor" != "$last_vendor" ]; then
    gap="$(default_spacing "$last_vendor")"
    [ -n "$SPACING" ] && gap="$SPACING"
    if [ "${gap:-0}" -gt 0 ]; then
      echo "spacing ${gap}s after vendor $last_vendor"
      sleep "$gap"
    fi
  fi
  if [ -n "$last_vendor" ] && [ "$vendor" = "$last_vendor" ]; then
    gap="$(default_spacing "$vendor")"
    [ -n "$SPACING" ] && gap="$SPACING"
    if [ "${gap:-0}" -gt 0 ]; then
      echo "spacing ${gap}s within vendor $vendor"
      sleep "$gap"
    fi
  fi
  run_entry "$slug" "$ROOT/$fixture" "$prefix" "$paid" "$vendor" || true
  last_vendor="$vendor"
done < <(read_entries | sort -t$'\t' -k5,5 -k1,1)

echo ""
echo "integration smoke summary: passed=$passed failed=$failed skipped=$skipped"
if [ "${#FAILURES[@]}" -gt 0 ]; then
  printf '  failures: %s\n' "${FAILURES[*]}"
fi

[ "$failed" -eq 0 ]
