#!/usr/bin/env bash
# Publish workspace crates leaf → root. Skips versions already on crates.io.
# Usage: publish-crates.sh [--dry-run]
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

dry_run=0
if [[ "${1:-}" == "--dry-run" ]]; then
  dry_run=1
fi

# Keep in sync with docs/PUBLISHING.md.
crates=(
  stackless-core
  stackless-git
  stackless-stripe-projects
  stackless-provider-sdk
  stackless-cloud
  stackless-daemon
  render-client
  vercel-client
  stackless-local
  stackless-render
  stackless-vercel
  stackless-fly
  stackless-netlify
  stackless-railway
  stackless-laravel-cloud
  stackless-gitlab
  stackless-wordpress
  stackless-cloudflare
  stackless-integrations
  stackless-idl
  stackless-bindgen
  stackless
)

publish_one() {
  local crate="$1"
  local args=(-p "$crate")
  if [[ "$dry_run" -eq 1 ]]; then
    args+=(--dry-run)
  fi

  local log
  log="$(mktemp)"
  set +e
  cargo publish "${args[@]}" 2>&1 | tee "$log"
  local status=${PIPESTATUS[0]}
  set -e

  if [[ "$status" -eq 0 ]]; then
    rm -f "$log"
    return 0
  fi
  if grep -qiE 'already exists|already uploaded|is already uploaded' "$log"; then
    echo "skip $crate (already published at this version)"
    rm -f "$log"
    return 0
  fi
  # dry-run can still "succeed" packaging; treat other failures as hard.
  echo "error: cargo publish -p $crate failed" >&2
  rm -f "$log"
  return "$status"
}

for crate in "${crates[@]}"; do
  echo "==> $crate"
  publish_one "$crate"
done

echo "crates.io wave complete (dry_run=$dry_run)"
