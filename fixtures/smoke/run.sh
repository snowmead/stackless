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

# Optional provider: skip when explicitly unconfigured (Fly is paid + needs link).
if [ "$substrate" = "fly" ] && [ -z "${FLY_API_TOKEN:-}" ]; then
  echo "skip live-smoke (fly): FLY_API_TOKEN not configured"
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

up=0
# shellcheck disable=SC2086  # $extra is intentionally word-split
cargo run -q -p stackless -- up --name "$inst" --on "$substrate" --file "$fixture" $extra --json | assert_json_ok || up=$?

# Live coverage for the machine contract on a real substrate: status and logs
# must emit ok:true envelopes (logs may report per-service source:"unavailable"
# but the envelope itself must be well-formed). Only gated when up succeeded.
post=0
if [ "$up" -eq 0 ]; then
  cargo run -q -p stackless -- status "$inst" --json | assert_json_ok || post=$?
  cargo run -q -p stackless -- logs "$inst" --tail 20 --json | assert_json_ok || post=$?
fi

# Teardown always runs; verified-gone is part of `down`. Exit non-zero if either
# the up (health gate), the JSON contract checks, or the down (teardown) failed.
down=0
cargo run -q -p stackless -- down "$inst" --json | assert_json_ok || down=$?

exit $(( up != 0 || post != 0 || down != 0 ))
