#!/usr/bin/env bash
# Fail closed if language SDK / workspace versions disagree with an optional tag.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

workspace_version="$(
  sed -n '/^\[workspace\.package\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)"/\1/p;}' Cargo.toml \
    | head -n1
)"
ts_version="$(node -p "require('./sdks/typescript/package.json').version")"
py_version="$(
  sed -n '/^\[project\]/,/^\[/{s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)"/\1/p;}' sdks/python/pyproject.toml \
    | head -n1
)"

if [[ -z "$workspace_version" || -z "$ts_version" || -z "$py_version" ]]; then
  echo "error: could not read versions (workspace='$workspace_version' ts='$ts_version' py='$py_version')" >&2
  exit 1
fi

if [[ "$ts_version" != "$workspace_version" || "$py_version" != "$workspace_version" ]]; then
  echo "error: version mismatch — workspace=$workspace_version npm=$ts_version pypi=$py_version" >&2
  exit 1
fi

echo "lockstep version: $workspace_version"

if [[ "${1:-}" == "--tag" ]]; then
  tag="${2:-}"
  if [[ -z "$tag" ]]; then
    echo "error: --tag requires a tag name" >&2
    exit 1
  fi
  # Accept v0.1.7 or bare 0.1.7
  tag_version="${tag#v}"
  if [[ "$tag_version" != "$workspace_version" ]]; then
    echo "error: tag $tag does not match workspace version $workspace_version" >&2
    exit 1
  fi
  echo "tag ok: $tag"
fi
