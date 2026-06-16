#!/usr/bin/env bash
# Shared live-smoke runner for every provider. Fixes the bumps the per-provider
# tasks hit: (1) ALWAYS tears down even if `up` fails (`|| up=$?` instead of the
# errexit-skipping `cmd; up=$?`), so a failed run never orphans paid resources;
# (2) a UNIQUE per-run instance name, so a re-run never collides with a resource
# that lingered provider-side after a prior teardown.
#
# Usage: run.sh <substrate> <fixture> <name-prefix>
#   substrate   --on target (vercel | render | local)
#   fixture     path to the smoke stackless.toml
#   name-prefix short instance-name prefix (e.g. smoke-v)
set -u

substrate="$1"
fixture="$2"
prefix="$3"

# Local env file for creds (CI injects them as env vars instead).
[ -f .stackless.env ] && { set -a; . ./.stackless.env; set +a; }

inst="${prefix}-$(date +%s)"

up=0
cargo run -q -p stackless -- up --name "$inst" --on "$substrate" --file "$fixture" || up=$?

# Teardown always runs; verified-gone is part of `down`. Exit non-zero if either
# the up (health gate) or the down (teardown) failed.
down=0
cargo run -q -p stackless -- down "$inst" || down=$?

exit $(( up != 0 || down != 0 ))
