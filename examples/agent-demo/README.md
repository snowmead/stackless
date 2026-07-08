# stackless agent demo

Minimal stack for agents: one static web service on the **local** substrate. No Docker, no secrets, no cloud credentials.

## Prerequisites

- [stackless](https://github.com/snowmead/stackless) installed:

  ```console
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh
  stackless doctor --json
  ```

- `python3` on `PATH`
- Run commands from the **repository root** (clone [github.com/snowmead/stackless](https://github.com/snowmead/stackless); the definition materializes `examples/agent-demo/site` from the `main` ref)

## Commands

Validate the definition:

```console
stackless check examples/agent-demo/stackless.toml
```

Bring the stack up (creates instance `demo`):

```console
stackless up --name demo --on local --file examples/agent-demo/stackless.toml
```

If `examples/agent-demo` is not yet on the pinned `main` ref (e.g. testing before merge), pin the site in place:

```console
stackless up --name demo --on local --file examples/agent-demo/stackless.toml --source web=examples/agent-demo/site
```

Expected human output ends with origins, e.g. `web: http://demo.localhost:4444`.

Prove health and renew the lease:

```console
stackless verify demo
```

Inspect state:

```console
stackless status demo
stackless list
```

Tear down (verified gone):

```console
stackless down demo
```

## JSON (`--json`)

Machine-readable success goes to **stdout** only; progress during `up --json` streams on **stderr** as NDJSON (`step_started`, `step_completed`, …) with `at_epoch_ms` and per-step `duration_ms`. Parse stdout for the final envelope.

Bring up:

```console
stackless up --name demo --on local --file examples/agent-demo/stackless.toml --json
```

```json
{
  "ok": true,
  "schema_version": 1,
  "instance": "demo",
  "executed": ["materialize:web", "start:web", "health:web"],
  "skipped": [],
  "origins": { "web": "http://demo.localhost:4444" },
  "duration_ms": 540,
  "steps": [
    { "id": "materialize:web", "duration_ms": 1 },
    { "id": "start:web", "duration_ms": 2 },
    { "id": "health:web", "duration_ms": 536 }
  ]
}
```

Verify (captures script output to a log file under state; envelope includes `log_path`, `exit_status`, `duration_ms`):

```console
stackless verify demo --json
stackless verify demo --tier smoke --json
```

Status and list:

```console
stackless status demo --json
stackless list --json
```

Logs (local substrate tails daemon service logs; unsupported substrates return `source: "unavailable"` with a `reason`):

```console
stackless logs demo --json
```

Tear down:

```console
stackless down demo --json
```

```json
{
  "ok": true,
  "schema_version": 1,
  "instance": "demo",
  "outcome": "destroyed"
}
```

Check and doctor:

```console
stackless check examples/agent-demo/stackless.toml --json
stackless doctor --file examples/agent-demo/stackless.toml --json
```

On failure, branch on `error.code` (never message text), e.g. `state.lock.held`, `local.health_failed`, `verify.failed`, `verify.tier_unknown`. Failed verify includes `context.log_path` and `context.log_tail`.

## What this proves

- Parse and validate a `stackless.toml`
- Materialize git source, start a process, route through the local reverse proxy
- Health-gate `up` on a real HTTP response
- Idempotent resume, `verify`, and verified teardown

Cloud substrates (Render, Vercel, Fly, Netlify) need provider setup; see [docs/SELFTEST.md](../../docs/SELFTEST.md) and `fixtures/smoke/`.
