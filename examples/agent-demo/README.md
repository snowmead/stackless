# stackless agent demo

Minimal stack for agents: one static web service on the **local** substrate. No Docker, no secrets, no cloud credentials.

## Prerequisites

- [stackless](https://github.com/snowmead/stackless) installed (`cargo install --path crates/stackless` from a clone, or a release binary once published)
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

Machine-readable success (`stdout`):

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
  "origins": {
    "web": "http://demo.localhost:4444"
  }
}
```

Progress events stream on **stderr** as NDJSON during `up --json` (`step_started`, `step_completed`, …). Parse `stdout` only for the final envelope.

Check without prose:

```console
stackless check examples/agent-demo/stackless.toml --json
stackless status demo --json
stackless down demo --json
```

On failure, branch on `error.code` (never message text), e.g. `state.lock.held`, `local.health_failed`.

## What this proves

- Parse and validate a `stackless.toml`
- Materialize git source, start a process, route through the local reverse proxy
- Health-gate `up` on a real HTTP response
- Idempotent resume, `verify`, and verified teardown

Cloud substrates (Render, Vercel, Fly, Netlify) need provider setup; see [docs/SELFTEST.md](../../docs/SELFTEST.md) and `fixtures/smoke/`.
