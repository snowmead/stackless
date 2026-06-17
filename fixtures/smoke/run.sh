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

inst="${prefix}-$(date +%s)"

up=0
# shellcheck disable=SC2086  # $extra is intentionally word-split
cargo run -q -p stackless -- up --name "$inst" --on "$substrate" --file "$fixture" $extra || up=$?

# Teardown always runs; verified-gone is part of `down`. Exit non-zero if either
# the up (health gate) or the down (teardown) failed.
down=0
cargo run -q -p stackless -- down "$inst" || down=$?

exit $(( up != 0 || down != 0 ))
