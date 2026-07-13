#!/usr/bin/env bash
# List Stripe Projects vendors required by integration smokes and whether they are linked.
set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MANIFEST="$ROOT/fixtures/smoke/integrations/manifest.json"

[ -f "$ROOT/.stackless.env" ] && { set -a; . "$ROOT/.stackless.env"; set +a; }

if ! command -v stripe >/dev/null 2>&1; then
  echo "::error::stripe CLI not installed"
  exit 1
fi

if [ -z "${STRIPE_API_KEY:-}" ]; then
  echo "::error::STRIPE_API_KEY unset — add to .stackless.env or environment" >&2
  exit 1
fi

export STRIPE_API_KEY

python3 - "$MANIFEST" <<'PY'
import json, os, subprocess, sys

manifest = json.load(open(sys.argv[1]))
required = sorted(set(e["vendor"] for e in manifest["integrations"]))

linked = set()
try:
    out = subprocess.run(
        ["stripe", "projects", "status"],
        capture_output=True,
        text=True,
        check=False,
    )
    text = (out.stdout or "") + (out.stderr or "")
    import re
    if text.strip().startswith("{"):
        data = json.loads(text)
        providers = (
            data.get("data", {}).get("providers")
            or data.get("providers")
            or []
        )
        for p in providers:
            name = (p.get("name") or p.get("provider") or "").lower()
            if name:
                linked.add(name)
    else:
        for line in text.splitlines():
            m = re.search(r"[✓✔]\s+([A-Za-z0-9._-]+)", line)
            if m:
                linked.add(m.group(1).lower())
except Exception as exc:
    print(f"warn: could not parse stripe projects status: {exc}", file=sys.stderr)

missing = [v for v in required if v not in linked]
print(f"required vendors: {len(required)}")
print(f"linked vendors:   {len(linked)}")
print(f"missing vendors:  {len(missing)}")
if missing:
    print("\nRun once per vendor (interactive OAuth):")
    for v in missing:
        print(f"  stripe projects link {v}")
    sys.exit(1 if os.environ.get("STACKLESS_LIVE_SMOKE_REQUIRED") == "1" else 0)
print("all required vendors appear linked")
PY
