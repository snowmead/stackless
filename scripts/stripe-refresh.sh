#!/usr/bin/env bash
set -euo pipefail
[ -f .stackless.env ] && { set -a; . ./.stackless.env; set +a; }
# The Stripe runner uses the CLI helper, including when called from a test binary.
STACKLESS_BIN=$(cargo build -p stackless --bin stackless --message-format=json | python3 -c '
import json, sys
artifacts = [json.loads(line) for line in sys.stdin]
executables = [item["executable"] for item in artifacts
               if item.get("reason") == "compiler-artifact"
               and item["target"]["name"] == "stackless" and item.get("executable")]
assert len(executables) == 1, "cargo did not report the stackless executable"
print(executables[0])
')
export STACKLESS_BIN
STRIPE_PROJECTS_REFRESH=1 cargo nextest run -p stackless-stripe-projects --all-features -E 'test(refresh_blesses_snapshots)' --no-capture
