#!/usr/bin/env bash
# Install tools from mise.toml, then force-retry on failure.
#
# Callers should set MISE_JOBS=1 (serial) to avoid mise's parallel rustup
# component races on a cold cache. Retries use --force to recover from
# corrupt/partial installs, but current mise requires explicit TOOL@VERSION
# arguments with --force (bare `mise install --force` fails clap parsing).
set -euo pipefail

if mise install; then
  exit 0
fi

mapfile -t tools < <(
  mise ls --current --json \
    | jq -r 'to_entries[] | select((.value | length) > 0) | "\(.key)@\(.value[0].requested_version)"'
)

for i in 1 2; do
  echo "retry $i (force)"
  sleep 10
  if ((${#tools[@]} > 0)); then
    if mise install --force "${tools[@]}"; then
      exit 0
    fi
  elif mise install; then
    exit 0
  fi
done

exit 1
