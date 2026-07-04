#!/bin/sh
# Serve static content regardless of materialized checkout layout.
ROOT="$(cd "$(dirname "$0")" && pwd)"
exec python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$ROOT/site"
