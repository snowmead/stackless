#!/usr/bin/env bash
# Hermetic agent-demo lifecycle gate: every verb emits parseable JSON with ok: true.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

eval "$(mise activate bash)" 2>/dev/null || true
BIN="${STACKLESS_BIN:-./target/debug/stackless}"
DEF="examples/agent-demo/stackless.toml"
SITE="examples/agent-demo/site"
NAME="agent-demo-ci-$$"

cleanup() {
  "$BIN" down "$NAME" --json >/dev/null 2>&1 || true
}
trap cleanup EXIT

cargo build -q -p stackless

assert_json_ok() {
  local label="$1"
  local json="$2"
  echo "$json" | python3 -c '
import json, sys
doc = json.load(sys.stdin)
assert doc.get("ok") is True, doc
'
  echo "ok: $label"
}

assert_json_ok check "$("$BIN" check "$DEF" --on local --json)"
assert_json_ok doctor "$("$BIN" doctor --file "$DEF" --json)"
assert_json_ok up "$("$BIN" up --name "$NAME" --on local --file "$DEF" --source web="$SITE" --json)"
assert_json_ok verify "$("$BIN" verify "$NAME" --json)"
assert_json_ok status "$("$BIN" status "$NAME" --json)"
assert_json_ok logs "$("$BIN" logs "$NAME" --tail 10 --json)"
assert_json_ok down "$("$BIN" down "$NAME" --json)"

echo "agent-demo lifecycle: all verbs returned ok: true on stdout"
