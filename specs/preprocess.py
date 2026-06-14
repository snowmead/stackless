#!/usr/bin/env python3
"""Make a provider OpenAPI spec progenitor/typify-friendly and minimal.

progenitor (via typify) only resolves top-level component `$ref`s of the form
`#/components/<section>/<name>`. Real-world specs (Render's `*WithCursor`
wrappers) use JSON-pointer `$ref`s into nested `properties`/`oneOf` — typify
panics on those. We also only use a handful of endpoints, so we trim.

Pipeline: keep only the requested paths -> inline every "deep" `$ref` (anything
not matching a clean top-level component ref) -> prune component schemas no
longer reachable from the kept paths. Output is a small spec progenitor accepts.

Usage:
  preprocess.py <in_spec.json> <out_spec.json> <path1> [<path2> ...]
The paths are exact OpenAPI path keys to keep (e.g. /services).
"""

import copy
import json
import re
import sys
from urllib.parse import unquote

CLEAN_REF = re.compile(
    r"^#/components/(schemas|responses|parameters|requestBodies|headers)/[^/]+$"
)


def resolve_pointer(root, ref):
    parts = ref[2:].split("/")
    cur = root
    for raw in parts:
        key = unquote(raw).replace("~1", "/").replace("~0", "~")
        cur = cur[int(key)] if isinstance(cur, list) else cur[key]
    return cur


def inline_deep_refs(node, root, depth=0):
    if depth > 200:
        raise RuntimeError("ref inlining too deep — possible cycle")
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str) and ref.startswith("#/") and not CLEAN_REF.match(ref):
            try:
                target = copy.deepcopy(resolve_pointer(root, ref))
            except (KeyError, IndexError, ValueError):
                # Unresolvable deep ref — only exists in schemas we don't keep;
                # leave it so pruning drops it (typify never sees it).
                return node
            return inline_deep_refs(target, root, depth + 1)
        return {k: inline_deep_refs(v, root, depth) for k, v in node.items()}
    if isinstance(node, list):
        return [inline_deep_refs(x, root, depth) for x in node]
    return node


def collapse_multi_success(paths):
    """progenitor requires <=1 success response type per operation. When an
    operation declares multiple 2xx responses, keep one — preferring a
    no-content response (e.g. Render's create-deploy `202 Queued` empty body,
    which is what the live API actually returns), else the lowest status."""
    for item in paths.values():
        for op in item.values():
            if not isinstance(op, dict):
                continue
            responses = op.get("responses", {})
            ok = sorted(k for k in responses if k.startswith("2"))
            if len(ok) <= 1:
                continue
            empty = [k for k in ok if "content" not in responses[k]]
            keep = empty[0] if empty else ok[0]
            for k in ok:
                if k != keep:
                    del responses[k]


def relax_string_enums(node):
    """Drop `enum` constraints on string schemas so the generated client
    deserializes responses tolerantly. A closed generated enum would fail to
    deserialize ANY response containing a value the provider added later (a new
    region, service type, deploy status, ...), breaking calls that only read a
    couple of fields. We keep our own Unknown-tolerant domain enums in the
    adapter instead. (Integer/other enums are left alone.)"""
    if isinstance(node, dict):
        if node.get("type") == "string" and "enum" in node:
            del node["enum"]
        for v in node.values():
            relax_string_enums(v)
    elif isinstance(node, list):
        for x in node:
            relax_string_enums(x)


def clear_required_lists(node):
    """Drop schema-level `required` arrays so every field deserializes as
    optional. The generated client then tolerates the provider omitting,
    adding, or renaming response fields (we only read a handful), and request
    bodies stay constructible. Only array-valued `required` (schema keyword) is
    cleared — the boolean `required` on parameters/requestBodies is left alone."""
    if isinstance(node, dict):
        if isinstance(node.get("required"), list):
            del node["required"]
        for v in node.values():
            clear_required_lists(v)
    elif isinstance(node, list):
        for x in node:
            clear_required_lists(x)


def clean_refs_in(node, out):
    if isinstance(node, dict):
        ref = node.get("$ref")
        if isinstance(ref, str) and CLEAN_REF.match(ref):
            out.add(tuple(ref[2:].split("/")[1:]))  # (section, name)
        for v in node.values():
            clean_refs_in(v, out)
    elif isinstance(node, list):
        for x in node:
            clean_refs_in(x, out)


def prune_components(spec):
    """Drop component schemas/params/etc. not reachable (transitively via clean
    refs) from the kept paths."""
    comps = spec.get("components", {})
    reachable = set()
    frontier = set()
    clean_refs_in(spec.get("paths", {}), frontier)
    while frontier:
        nxt = set()
        for section, name in frontier:
            if (section, name) in reachable:
                continue
            reachable.add((section, name))
            node = comps.get(section, {}).get(name)
            if node is not None:
                clean_refs_in(node, nxt)
        frontier = nxt - reachable

    for section in list(comps.keys()):
        if not isinstance(comps[section], dict):
            continue
        comps[section] = {
            name: node
            for name, node in comps[section].items()
            if (section, name) in reachable
        }
        if not comps[section]:
            del comps[section]


def main():
    in_path, out_path, *keep_paths = sys.argv[1:]
    spec = json.load(open(in_path))
    # Pristine copy for pointer resolution — trimming/inlining mutate `spec`.
    root = json.load(open(in_path))

    # 1. Trim paths.
    if keep_paths:
        kept = {p: spec["paths"][p] for p in keep_paths if p in spec["paths"]}
        missing = [p for p in keep_paths if p not in spec["paths"]]
        if missing:
            print(f"WARNING: paths not in spec: {missing}", file=sys.stderr)
        spec["paths"] = kept

    # 1b. Collapse operations with multiple 2xx responses to one.
    collapse_multi_success(spec["paths"])

    # 1c. Inline deep refs FIRST — so the subtrees pulled in from `root` are
    #     also relaxed below (otherwise enums/`required` inside an inlined
    #     deep-ref escape relaxation). Pointers resolve against the pristine
    #     `root`; inline across paths + all components (some are referenced via
    #     deep pointers), pruning happens last.
    spec["paths"] = inline_deep_refs(spec["paths"], root)
    if "components" in spec:
        spec["components"] = inline_deep_refs(spec["components"], root)

    # 1d. Relax string enums to plain strings (drift-tolerant generated client).
    relax_string_enums(spec.get("paths", {}))
    relax_string_enums(spec.get("components", {}))

    # 1e. Drop `required` arrays so response fields deserialize as optional
    #     (tolerant to provider field changes; only a few fields are read).
    clear_required_lists(spec.get("paths", {}))
    clear_required_lists(spec.get("components", {}))

    # 2. Prune components to those reachable (transitively) from kept paths.
    prune_components(spec)

    json.dump(spec, open(out_path, "w"), indent=2)
    n_paths = len(spec["paths"])
    n_schemas = len(spec.get("components", {}).get("schemas", {}))
    print(f"wrote {out_path}: {n_paths} paths, {n_schemas} schemas")


if __name__ == "__main__":
    main()
