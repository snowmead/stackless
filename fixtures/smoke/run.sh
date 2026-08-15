#!/usr/bin/env bash
# Shared live-smoke runner for every provider. Fixes the bumps the per-provider
# tasks hit: (1) ALWAYS tears down even if `up` fails (`|| up=$?` instead of the
# errexit-skipping `cmd; up=$?`), so a failed run never orphans paid resources;
# (2) a UNIQUE per-run instance name, so a re-run never collides with a resource
# that lingered provider-side after a prior teardown.
#
# Usage: run.sh <substrate> <fixture> <name-prefix> [extra-up-flags...]
#   substrate    --on target (vercel | render | local | fly)
#   fixture      path to the smoke stackless.toml
#   name-prefix  short instance-name prefix (e.g. smoke-v)
#   extra-up-flags  optional flags appended to `up` (e.g. `--confirm-paid` for
#                   fly, whose flyio/app is usage-billed; bounded by the cap)
set -u

substrate="$1"
fixture="$2"
prefix="$3"
shift 3
# Optional extra `up` flags (e.g. --confirm-paid). Unquoted on use so a flag
# word-splits; empty when absent (bash 3.2 + set -u safe, unlike an empty array).
extra="$*"

# Local env file for creds (CI injects them as env vars instead).
[ -f .stackless.env ] && { set -a; . ./.stackless.env; set +a; }

if [ "${STACKLESS_LIVE_SMOKE_REQUIRED:-}" = "1" ] && [ -z "${STRIPE_API_KEY:-}" ]; then
  echo "::error::STRIPE_API_KEY is required for live smoke (configure repo secret or .stackless.env)"
  exit 1
fi

# Optional providers: skip when explicitly unconfigured.
if [ "$substrate" = "fly" ] && [ -z "${FLY_API_TOKEN:-}" ]; then
  echo "skip live-smoke (fly): FLY_API_TOKEN not configured"
  exit 0
fi
if [ "$substrate" = "netlify" ] && [ -z "${NETLIFY_AUTH_TOKEN:-}" ]; then
  echo "skip live-smoke (netlify): NETLIFY_AUTH_TOKEN not configured (run stripe projects link netlify or set the secret)"
  exit 0
fi
if [ "$substrate" = "railway" ] && [ -z "${RAILWAY_TOKEN:-}${RAILWAY_API_TOKEN:-}" ]; then
  echo "skip live-smoke (railway): RAILWAY_TOKEN not configured (run stripe projects link railway or set the secret)"
  exit 0
fi
if [ "$substrate" = "cloudflare" ] && [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  echo "skip live-smoke (cloudflare): CLOUDFLARE_API_TOKEN not configured (run stripe projects link cloudflare or set the secret)"
  exit 0
fi
if [ "$substrate" = "wordpress" ] && [ -z "${WORDPRESS_COM_ACCESS_TOKEN:-}${WORDPRESS_ACCESS_TOKEN:-}" ]; then
  echo "skip live-smoke (wordpress): WORDPRESS_COM_ACCESS_TOKEN not configured"
  exit 0
fi
if [ "$substrate" = "laravel-cloud" ] && [ -z "${LARAVEL_CLOUD_API_TOKEN:-}" ]; then
  echo "skip live-smoke (laravel-cloud): LARAVEL_CLOUD_API_TOKEN not configured"
  exit 0
fi
if [ "$substrate" = "gitlab" ] && [ -z "${GITLAB_TOKEN:-}${GITLAB_ACCESS_TOKEN:-}" ]; then
  echo "skip live-smoke (gitlab): GITLAB_TOKEN not configured (run stripe projects link gitlab or set the secret)"
  exit 0
fi

inst="${prefix}-$(date +%s)"

# Assert a verb's --json stdout parses and carries ok: true. Non-fatal input
# handling stays in the caller (each callsite decides whether failure gates).
assert_json_ok() {
  python3 -c '
import json, sys
doc = json.load(sys.stdin)
assert doc.get("ok") is True, doc
'
}

# Expected `logs` provenance for cloud hosts that implement fetch_logs.
# Empty means "ok:true envelope only" (e.g. local / Phase-1 hosts).
expected_logs_source() {
  case "$1" in
    vercel) echo vercel_api ;;
    fly) echo fly_events ;;
    netlify) echo netlify_api ;;
    render) echo render_api ;;
    railway) echo railway_api ;;
    cloudflare) echo cloudflare_api ;;
    wordpress) echo wordpress_api ;;
    laravel-cloud) echo laravel_cloud_api ;;
    gitlab) echo gitlab_api ;;
    *) echo "" ;;
  esac
}

# Assert logs --json: ok, every service carries the expected source, and at
# least one service returns a non-empty window (DoD for listed cloud hosts).
assert_logs_window() {
  local expected_source="$1"
  python3 -c '
import json, sys
expected = sys.argv[1]
doc = json.load(sys.stdin)
assert doc.get("ok") is True, doc
services = doc.get("services") or []
assert services, ("no services in logs envelope", doc)
for svc in services:
    assert svc.get("source") == expected, (svc, expected)
assert any(svc.get("lines") for svc in services), ("empty log window", doc)
' "$expected_source"
}

up=0
# shellcheck disable=SC2086  # $extra is intentionally word-split
cargo run -q -p stackless -- up --name "$inst" --on "$substrate" --file "$fixture" $extra --json | assert_json_ok || up=$?

# Live coverage for the machine contract on a real substrate: status must emit
# ok:true; logs must emit ok:true, and for hosts with wired fetch_logs the
# envelope must carry the expected source plus a non-empty window. Only gated
# when up succeeded.
post=0
if [ "$up" -eq 0 ]; then
  cargo run -q -p stackless -- status "$inst" --json | assert_json_ok || post=$?
  logs_source="$(expected_logs_source "$substrate")"
  if [ -n "$logs_source" ]; then
    logs_ok=1
    for _try in 1 2 3 4 5 6; do
      if cargo run -q -p stackless -- logs "$inst" --tail 20 --json | assert_logs_window "$logs_source"; then
        logs_ok=0
        break
      fi
      sleep 10
    done
    post=$((post | logs_ok))
  else
    cargo run -q -p stackless -- logs "$inst" --tail 20 --json | assert_json_ok || post=$?
  fi
fi

# Teardown always runs; verified-gone is part of `down`. Exit non-zero if either
# the up (health gate), the JSON contract checks, or the down (teardown) failed.
down=0
cargo run -q -p stackless -- down "$inst" --json | assert_json_ok || down=$?

exit $(( up != 0 || post != 0 || down != 0 ))
