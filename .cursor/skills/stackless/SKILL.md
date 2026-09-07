---
name: stackless
description: >-
  Author stackless.toml and run the stackless CLI lifecycle with structured
  JSON results. Use for local or cloud stacks, workload placement, jobs,
  endpoints, verification, recovery, and teardown.
---

# stackless agent skill

Use `--json` and branch on `error.code`. The controller owns execution and
cleanup. CLI, SDK, and MCP callers submit operations and inspect their results.

For the current model, read [the schema](../../../docs/SCHEMA.md) and
[execution support](../../../docs/EXECUTION.md). Use [the SDK protocol](../../../sdks/PROTOCOL.md)
for envelope fields and [the README](../../../README.md) for installation.
Inside this checkout, build with `cargo build` and use `target/debug/stackless`.
A released binary may precede this checkout's schema changes.

## Definitions

- `services` and `workloads` name the same table. A service requires HTTP
  `health = { path = "/" }` or supported TCP health. Workers use
  `kind = "worker"` and may omit health. Jobs use `[jobs.<name>]` and finish
  with an exit code; they cannot declare health.
- Common `run` and `env` fields work without a local provider block.
  Sources can use `repo` plus `ref`, or a supported local `path`.
  `source.root` selects a relative directory inside the source.
- Image and source-free support varies by provider. `check --on` rejects
  unsupported combinations before creation. Fly and Railway support common
  images; native Fly jobs remain unsupported.
- `--on` supplies the default provider. Individual workloads and integrations
  can set `on`. Mixed stacks run one dependency graph. Placement cannot change
  while that workload has retained resources or checkpoints.
- `depends_on = { database = "ready", migrate = "completed" }` expresses
  readiness and job completion. `started` is also supported. URL references
  wait for their output when necessary; they do not imply readiness.
- Endpoints name workload URLs: `[endpoints.api]` with `workload = "web"`.
  `${endpoints.api.url}` uses that binding. An explicit endpoint `url` is
  caller-managed and remains unverified. It does not create a route.
- Local host TCP listeners use `health = { protocol = "tcp" }`, listen on
  their injected `PORT`, and return `tcp://127.0.0.1:<port>` after start.
  TCP readiness proves a connection. Use a job or verification for protocol
  assertions. Cloud and isolated-container TCP routes are unsupported.
- `--source service=path` selects a caller-owned checkout for a workload on
  a provider that supports local pins, including local workloads in mixed
  stacks. Use `--dirty` when a snapshot is required.

## Execution authority

Host processes and cloud prepare hooks require the caller's
`--allow-host-execution` grant. A definition cannot grant this authority.
The grant belongs to the instance birth; reusing a name does not inherit it.
Use existing session authorization when deciding whether to pass the flag.

Supported local image workloads run in isolated containers. Host execution is
trusted code execution, not a sandbox. Do not claim host filesystem or network
isolation from a resource name or lease.

Stripe Projects owns catalog provisioning. Runtime context and vault files are
controller-managed. Do not switch Stripe environments, rewrite project anchors,
or scrape vault files to work around a failed stackless operation.

## Lifecycle

For a new local host stack, after host execution is authorized:

```sh
stackless check stackless.toml --on local --json
stackless doctor --file stackless.toml --on local --json
stackless up --name demo --on local --allow-host-execution --json
stackless status demo --json
stackless verify demo --json
stackless logs demo --tail 100 --json
stackless down demo --json
```

`verify` requires `[stack.verify]`; named tiers use `--tier`. Omit verification
when the definition has no verification command. Creation can use `--file`.
Resume with `up --name demo`; pass `--file` to reconcile changed inputs.

Cloud placement requires its provider credentials. Pass `--confirm-paid` when
paid creation is authorized. A sleeping operator cannot enforce cloud expiry;
use an always-on controller for unattended leases.

For caller-independent work, submit `up`, `down`, or `verify` with `--no-wait`.
Use `operation get <id> --after <cursor> --json` to reconnect and
`operation cancel <id> --json` to request cancellation. Retain the operation ID.
A lost response does not authorize another creation under a different identity.
Remote execution uses `--controller ssh://user@host` and that host's controller,
not a shared database opened by several lifecycle writers.

## Results and recovery

- Current envelopes use `schema_version: 2`. Successful `up` includes the
  immutable `instance_id`, executed and skipped steps, `origins`,
  `endpoints`, resolved `placements`, and integration outputs.
- Secret integration outputs are scoped `secret_ref` objects. They are not
  plaintext credentials. Inject them into workload or verification environments
  with `${integrations.<name>.<output>}`. Public outputs remain plain values.
- Endpoint strings follow the workload protocol. Do not send HTTP requests to
  a TCP URL. `source: "declared"` does not establish readiness.
- Status separates desired/applied revisions, existence, configuration, and
  readiness. Unknown observations do not establish absence.
- Status `resources` contains retained inventory metadata without provider
  payloads. `desired: false` identifies work removed from the definition.
  `phase` is recorded progress; `has_checkpoint` does not prove the current
  desired revision completed.
- After a provider failure, inspect the operation and retained resources, then
  resume or tear down through the controller. Preserve ambiguous creation and
  deletion evidence. Do not edit the database to force success.
- Treat nonzero `down` as incomplete teardown. Confirmed absence and Stripe
  deregistration are separate facts.

## Typed bindings

Generate bindings after the definition stabilizes:

```sh
stackless bind --file stackless.toml \
  --idl .stackless/stack.idl.json \
  --emit typescript=e2e/stack.gen.ts \
  --emit rust=tests/support/stack_bind.rs
```

Rust, TypeScript, Python, and Go generators expose origin and endpoint bindings,
integration references, required secret names, and verification tiers. SDK URL
map helpers feed the generated endpoint binders. Bindings are not lifecycle clients.
