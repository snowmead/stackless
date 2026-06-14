#!/usr/bin/env python3
"""Replace Vercel's pathologically large inline request/response schemas with
minimal ones covering exactly the fields stackless sends/reads.

Vercel's OpenAPI spec inlines enormous deployment/project schemas (the trimmed
spec is still ~1.5MB and progenitor emits ~145k lines). We only send/read a
handful of fields, so we swap those operation bodies for minimal schemas. The
result is a small, typed client over the official paths.

Usage: vercel_minimize.py <in_spec.json> <out_spec.json>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from preprocess import prune_components

STR = {"type": "string"}


def obj(**props):
    return {"type": "object", "properties": dict(props)}


DEPLOY_REQUEST = obj(
    name=STR,
    project=STR,
    gitSource=obj(type=STR, org=STR, repo=STR, ref=STR),
    projectSettings=obj(
        framework=STR,
        buildCommand=STR,
        installCommand=STR,
        rootDirectory=STR,
        outputDirectory=STR,
    ),
)
DEPLOYMENT = obj(id=STR, url=STR, readyState=STR, status=STR)
PROJECT = obj(id=STR, name=STR)
PROJECT_LIST = obj(projects={"type": "array", "items": PROJECT})
ENV_BODY = obj(key=STR, value=STR, type=STR, target={"type": "array", "items": STR})
EMPTY = {"type": "object"}


def set_request(op, schema):
    op["requestBody"]["content"]["application/json"]["schema"] = schema


KEEP_QUERY = {"search", "teamId", "slug", "upsert"}


def strip_params(op):
    """Keep only path params and the few query params we actually pass."""
    params = op.get("parameters")
    if not params:
        return
    op["parameters"] = [
        p for p in params if p.get("in") == "path" or p.get("name") in KEEP_QUERY
    ]


def set_2xx(op, schema):
    for status, resp in op.get("responses", {}).items():
        if status.startswith("2"):
            resp.setdefault("content", {}).setdefault("application/json", {})
            resp["content"]["application/json"]["schema"] = schema


def main():
    in_path, out_path = sys.argv[1:3]
    spec = json.load(open(in_path))
    paths = spec["paths"]

    set_request(paths["/v13/deployments"]["post"], DEPLOY_REQUEST)
    set_2xx(paths["/v13/deployments"]["post"], DEPLOYMENT)
    set_2xx(paths["/v13/deployments/{idOrUrl}"]["get"], DEPLOYMENT)
    set_2xx(paths["/v10/projects"]["get"], PROJECT_LIST)
    set_2xx(paths["/v9/projects/{idOrName}"]["get"], PROJECT)
    set_request(paths["/v10/projects/{idOrName}/env"]["post"], ENV_BODY)
    set_2xx(paths["/v10/projects/{idOrName}/env"]["post"], EMPTY)

    # Strip unused query parameters from every operation we keep.
    for item in paths.values():
        for op in item.values():
            if isinstance(op, dict):
                strip_params(op)

    # Drop the now-orphaned giant component schemas, keeping only those still
    # reachable (e.g. via remaining query parameters).
    prune_components(spec)

    json.dump(spec, open(out_path, "w"), indent=2)
    n_schemas = len(spec.get("components", {}).get("schemas", {}))
    print(f"wrote {out_path}: {len(paths)} paths, {n_schemas} schemas")


if __name__ == "__main__":
    main()
