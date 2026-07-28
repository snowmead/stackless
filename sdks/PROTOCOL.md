# CLI JSON protocol for language SDKs

Non-Rust SDKs (`@stackless/sdk`, Python, Go) are **CLI-backed**. They spawn the
`stackless` binary with `--json` and parse stdout envelopes. This is not the
same as Rust `Client::system()`, which runs the Engine in-process and only uses
`STACKLESS_BIN` / `PATH` for daemon binary resolution.

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

Every success envelope includes `schema_version: 1` and `ok: true`.

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
  "schema_version": 1,
  "ok": true,
  "instance": "demo",
  "substrate": "local",
  "executed": ["…"],
  "skipped": ["…"],
  "duration_ms": 0,
  "steps": [],
  "origins": [{ "service": "web", "origin": "http://…" }],
  "integrations": {
    "clerk": { "secret_key": "sk_…", "publishable_key": "pk_…" }
  },
  "spend": null
}
```

`integrations` is omitted when empty. Values are credentials. Do not log the
raw envelope in CI without redaction. Prefer verify-tier env injection when
secrets must not appear on stdout.

Map for generated bind:

- origins: `{ [service]: origin }` → `bindOrigins`
- integrations: nested object as emitted → `bindIntegrations`

### `down`

Args: `<name>`

Success includes `instance`, `status` (`destroyed` | `already_down`), optional
`spend`.

### `verify`

Args: `<name>`, optional `--tier <dns>`

Success: `instance`, optional `tier`, `duration_ms`, `exit_status`, `log_path`,
optional `lease_remaining_secs`.

### `status`

Args: `<name>`

Success: flattened `InstanceReport` plus optional `persistence_warning`.

### `list`

Args: none

Success: `{ "ok": true, "instances": [ … ], "persistence_warning"?: "…" }`.

### `logs`

Args: `<name>`, optional `<service>`, `--tail <n>` (default 100)

Success: `name`, `substrate`, `available`, `services` array of log tails.

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

Typed `UpOutcome` must carry `instance`, `substrate`, `origins` map, and
`integrations` map suitable for generated bind helpers.

## Secrets policy

- Integration outputs appear only on `--json` success for `up` (never human text).
- Document that capturing `up --json` stdout captures credentials.
- Verify-tier `[stack.verify].env` with `${integrations.*.*}` remains the
  preferred prove path when Playwright runs as the verify command.
