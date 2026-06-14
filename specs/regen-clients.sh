#!/usr/bin/env bash
# Regenerate the committed provider client crates from the vendored OpenAPI
# specs. Requires `cargo install cargo-progenitor` and python3.
#
# Pipeline per provider: the specs/*-openapi.json are vendored (fetched by
# hand). preprocess.py trims to the endpoints we use, inlines deep $refs,
# collapses multi-2xx operations, relaxes string enums, and clears `required`.
# For Vercel, vercel_minimize.py then replaces its enormous inline
# deployment/project schemas with minimal ones (the trimmed-but-not-minimized
# Vercel spec generates ~145k lines). cargo-progenitor generates each crate and
# we copy src/lib.rs in. Portable bash 3.2 (no `mapfile`).
set -euo pipefail
cd "$(dirname "$0")/.."
SPECS=specs
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# --- Render ---
python3 "$SPECS/preprocess.py" "$SPECS/render-openapi.json" "$SPECS/render-trimmed.json" \
  /services /postgres "/postgres/{postgresId}/connection-info" \
  "/services/{serviceId}/env-vars" "/services/{serviceId}/routes" \
  "/services/{serviceId}/deploys" "/services/{serviceId}/deploys/{deployId}" /logs
cargo progenitor -i "$SPECS/render-trimmed.json" -o "$TMP/render-client" -n render-client --version 0.1.0
cp "$TMP/render-client/src/lib.rs" crates/render-client/src/lib.rs
echo "regenerated crates/render-client/src/lib.rs"

# --- Vercel ---
python3 "$SPECS/preprocess.py" "$SPECS/vercel-openapi.json" "$TMP/vercel-pre.json" \
  /v10/projects "/v9/projects/{idOrName}" "/v10/projects/{idOrName}/env" \
  /v13/deployments "/v13/deployments/{idOrUrl}"
python3 "$SPECS/vercel_minimize.py" "$TMP/vercel-pre.json" "$SPECS/vercel-trimmed.json"
cargo progenitor -i "$SPECS/vercel-trimmed.json" -o "$TMP/vercel-client" -n vercel-client --version 0.1.0
cp "$TMP/vercel-client/src/lib.rs" crates/vercel-client/src/lib.rs
echo "regenerated crates/vercel-client/src/lib.rs"

echo "done — run: cargo build -p render-client -p vercel-client"
