# JSON protocol for language SDKs

Published clients exist for Rust (`stackless` on crates.io), TypeScript and
Python (`stackless-sdk` on npm/PyPI), and Go (`github.com/snowmead/stackless/sdks/go`).
TypeScript, Python, and Go spawn the `stackless` binary with `--json` and parse
stdout envelopes. Rust `Client::system()` calls the same controller over its
Unix socket and uses `STACKLESS_BIN` / `PATH` to start it when needed. All
lifecycle execution happens in that controller.

## Binary resolution

1. `STACKLESS_BIN` if set and non-empty
2. Else `stackless` on `PATH`

## Invocation

```
<bin> <verb> … --json
```

`--json` is global. Prefer reading a single JSON object from **stdout**. On
failure, stdout may still carry `{ "ok": false, "error": … }`; otherwise use
stderr / exit status.

Working directory is the caller's cwd (definition paths are relative to it
unless absolute).

## Envelopes

Every success envelope includes `schema_version: 2` and `ok: true`.

Failure:

```json
{ "ok": false, "error": { "code": "<stable>", "message": "…", … } }
```

Branch on `error.code`. Surface `error.message` to the user.

### `up`

Args (creation):

- `--name <instance>` (optional; stack allocates when omitted)
- `--file <path>` (optional)
- `--on <substrate>` (required at creation)
- `--source SVC[=PATH]` (repeatable)
- `--dirty` (with `--source`)
- `--lease <duration>`
- `--confirm-paid`

Resume: `--name` of an existing instance; `--on` ignored.

Success (relevant fields):

```json
{
  "schema_version": 2,
  "ok": true,
  "instance": "demo",
  "instance_id": "immutable-birth-id",
  "substrate": "local",
  "executed": ["…"],
  "skipped": ["…"],
  "duration_ms": 0,
  "steps": [],
  "origins": [{ "service": "web", "origin": "http://…" }],
  "placements": { "workloads": { "web": "local" }, "resources": { "clerk": "local" } },
  "endpoints": {
    "public": { "workload": "web", "url": "https://example.com", "source": "declared" }
  },
  "integrations": {
    "clerk": { "secret_key": { "kind": "secret_ref", "instance_id": "immutable-birth-id", "integration": "clerk", "output": "secret_key" } }
  },
  "spend": null
}
```

Endpoint and origin strings follow the workload protocol. TCP workloads return
`tcp://127.0.0.1:<port>` after their listener port has been recorded. SDKs preserve
the scheme; callers must not assume every URL accepts HTTP requests. Declared
TCP endpoints retain `source: "declared"` and unknown readiness, like declared
HTTP endpoints.

`integrations` is omitted when empty. Every value is a `SecretRef` scoped to
`instance_id`. CLI, SDK, and MCP results contain these references. They contain
no credential values and cannot be converted to plaintext through the public
API. The SDKs reject plaintext values and references for another instance.

Inject credentials into declared service or verify environments with
`${integrations.<name>.<output>}`. The controller resolves these expressions.
Generated `bindOrigins` accepts the origins map. Generated `bindIntegrations`
accepts plaintext maps inside a workload that already received credentials;
it does not accept the controller's secret-reference map.

`substrate` is the default hosting adapter. `placements.workloads` and
`placements.resources` map logical names to resolved hosting adapters. Resource
hosting is separate from its catalog provider. SDKs expose these maps through
`Placements`; older results without the field produce empty maps. `check --on`
returns the same placement object. Status workload and integration entries
include their resolved `on` provider.

`endpoints` maps names to `{ workload, url, source }`. `source` is `provider` or
`declared`; declared URLs do not imply routing or successful probing. SDKs default
missing maps to empty for older operation results. Status endpoint entries have
an optional URL and `readiness`; declared URL readiness is always `unknown`.

Generated endpoint bindings accept URL maps: Rust `Endpoints::from_map` uses
`outcome.endpoint_urls()`, TypeScript `bindEndpoints` uses `endpointUrls(outcome)`,
Python `bind_endpoints` uses `outcome.endpoint_urls`, and Go `BindEndpoints` uses
`outcome.EndpointURLs()`. The IDL carries endpoint names and target workloads;
deployment URLs are supplied at runtime.

### `down`

Args: `<name>`

Success includes `instance`, `outcome` (`destroyed` | `already_down`), optional
`spend`.

### Durable operations

`up --no-wait` and `down <name> --no-wait` return an `operation` object.
The controller persists the request before acknowledging submission.

- `operation get <id> --after <cursor>` returns the operation and persisted events.
- `operation wait <id>` waits for its stored result or error.
- `operation cancel <id>` requests cancellation.
- `operation list --instance <name>` returns recent operations.

Cancellation of running `up` takes effect between steps. It leaves resource
evidence for resume or teardown. Running `down` and `verify` reject cancellation.
A controller restart retries unfinished `up` and `down` with their existing IDs.
An interrupted `verify` must be submitted again because proof commands may
have side effects. See [the controller contract](../docs/CONTROLLER.md).

### `verify`

Args: `<name>`, optional `--tier <dns>`

Success: `instance`, optional `tier`, `duration_ms`, `exit_status`, `log_path`,
optional `lease_remaining_secs`.

### `status`

Args: `<name>`

Success: flattened `InstanceReport` plus optional `persistence_warning`.

`resources` lists retained inventory entries, including resources whose workload
was removed from the desired definition. Each entry includes `key`, `step_id`,
hosting adapter `on`, catalog/native `provider`, `ownership`, `phase`,
`resource_kind`, `resource_id`, `dependencies`, `updated_at`, `has_checkpoint`,
and `desired`. Provider payloads and credentials are omitted. Confirmed-absent
entries stay in the controller journal and are omitted from this view.

`phase` is recorded progress. It is not a live existence or readiness observation.
`has_checkpoint` means the step has a completion record; that record may describe
an earlier revision. `desired: false` identifies retained work removed from the
definition. Clients talking to older servers should treat a missing `resources`
field as an empty list.

Service observations keep existence, configuration, and readiness separate.
Failed or blocked observations report unknown configuration even when the
last applied revision matches the desired revision. Revision history remains
available in the separate `revision` field.

### `list`

Args: none

Success: `{ "ok": true, "instances": [ … ], "persistence_warning"?: "…" }`.

### `logs`

Args: `<name>`, optional `<service>`, `--tail <n>` (default 100)

Success when logs are available: `instance`, `services` (array of log tails).
When unavailable: same fields plus `substrate` (no separate `available` flag).

### `check`

Args: `<file>`, optional `--on <substrate>`

Success: `stack`, optional `substrate`, `services`, `graph`.

## SDK surface (verb parity)

Each language client should expose:

| Method | CLI |
| --- | --- |
| `up(create \| resume)` | `up` |
| `down(name)` | `down` |
| `verify(name, tier?)` | `verify` |
| `status(name)` | `status` |
| `list()` | `list` |
| `logs(name, service?, tail?)` | `logs` |
| `check(file, on?)` | `check` |

Typed `UpOutcome` carries `instance`, immutable `instance_id`, `substrate`,
an `origins` map, an `endpoints` map, and an `integrations` map of typed secret references.

## Secrets policy

The controller retains redaction values across rotation, teardown, and restart.
Known values are removed from operation results, errors, and returned logs.
Legacy operation payloads without redaction history are withheld. Workload log
files and the controller database use owner-only permissions.

Provider administration credentials cannot be requested through the app secret
namespace. Workload, prepare, and verify processes inherit a controlled baseline
plus their explicitly resolved environment. This environment policy alone is
not a filesystem, process, or network sandbox.

Use `[stack.verify].env` with `${integrations.*.*}` when Playwright or another
verification job needs credentials.
