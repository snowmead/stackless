# Agent fleets

Running many stackless instances from parallel agents — Cursor cloud agents,
CI shards, or a local fleet — needs predictable isolation, shared state when
appropriate, and cost hygiene. This doc covers the three decisions every fleet
operator makes.

## Parallel agents: worktree-per-agent vs `--dirty`

stackless materializes git sources per instance. Two active instances cannot
share the same bare `--source` checkout without tripping
`engine.source_override.shared`.

**Preferred: one git worktree per agent.** Each agent gets its own checkout
path, its own `definition_dir`, and private controller-owned Stripe context. Name
instances distinctly (`agent-a-demo`, `pr-42-smoke`, …) and omit `--source` so
stackless clones from the pinned ref in `stackless.toml`.

**Alternative: `--source … --dirty` on a single checkout.** When agents must
run against an uncommitted tree, pin with `--source svc=PATH` and add
`--dirty` so stackless snapshots the working tree into instance-owned space.
This is explicit, recorded, local-only, and safe for parallel instances — but
heavier than a worktree and unsuitable for cloud substrates (cloud rejects
`--source` / `--dirty`; commit and push instead).

| Pattern | When to use |
|---------|-------------|
| Worktree per agent | Default for parallel cloud/local fleets |
| `--source` without `--dirty` | Single active instance per checkout (edit loop) |
| `--source --dirty` | Local parallel agents on one dirty tree |

See [ARCHITECTURE.md](../ARCHITECTURE.md) §2 (parallel `up`) for the locking
model.

## One controller for the fleet

Every agent submits operations to the same controller. It owns the SQLite file,
provider calls, operation queue, and lease reaper. Names are unique within that
controller. Different instances can run concurrently; operations on one instance
are serialized.

```bash
export STACKLESS_CONTROLLER="ssh://controller-host"
stackless controller --json
stackless list --json
```

The CLI, SDKs, and MCP use this transport. OpenSSH checks the host key and uses
the configured account. Install the controller on an always-on host for unattended
leases. See [controller operations](CONTROLLER.md) and the
[systemd service](../deploy/systemd/README.md).

`STACKLESS_STATE_URL` is rejected with `state.remote.disabled` when opening a
configured store. A shared database cannot supervise processes or prevent two
machines from executing the same provider action. Unset `STACKLESS_STATE_URL`
and `STACKLESS_STATE_TOKEN` after migrating old state.

### Migrate a legacy fleet database

Stop every old writer and reaper before taking a final export. Keep the original
database and runtime directories until all existing instances have been audited
and torn down. Local processes and their source paths belong to the original
host; moving their database records does not move the processes.

Create a private SQL dump and a new SQLite file using the
[Turso dump workflow](https://docs.turso.tech/local-development):

```bash
umask 077
stackless_migration_dir="$(mktemp -d)"
turso db shell DATABASE .dump > "$stackless_migration_dir/fleet.sql"
sqlite3 -bail "$stackless_migration_dir/state.db" < "$stackless_migration_dir/fleet.sql"
```

Use that `state.db` in a new controller state directory before its first startup.
Do not overwrite an existing controller database or combine files from different
controllers. Database files and dumps contain credentials and must remain private.

On open, Stackless verifies the exported table and column layout against
`_stackless_schema_version`. It converts the marker and applies later SQLite
migrations in one transaction. Partial exports and newer schemas fail without
rewriting the schema. Checkpoints, leases, foreign-host claims, and resource
identities remain recorded. Reopening does not assign another instance identity.

The conversion does not resolve foreign locks or establish ownership of legacy
cloud resources. Those audits remain required before claiming migration and
teardown conformance. The controller's Linux deployment and reboot survival also
still need live host verification.

## Naming conventions

Instance names become hostnames and cloud resource labels. stackless enforces
DNS-safe names: `^[a-z][a-z0-9-]*$`, max 63 characters.

Suggested patterns for fleets:

| Pattern | Example | Use |
|---------|---------|-----|
| `{agent}-{purpose}` | `cursor-demo` | Single agent, known stack |
| `{pr}-{sha-prefix}` | `pr42-a1b2c3` | CI preview per PR |
| `{stack}-{uuid}` | (auto when `--name` omitted) | Throwaway smokes |

Avoid reusing a name while an instance is still **active** on any substrate
owned by the same controller.

## Cost hygiene

Cloud substrates bill. stackless defaults to short leases (8h on render,
vercel, fly, netlify; 24h local) and requires **`--confirm-paid`** for paid
resources on each mutating invocation that spends money.

Fleet practices:

1. **`stackless down <name> --json` when done** — verified teardown; do not
   rely on lease expiry alone for cost-sensitive stacks.
2. **Set explicit leases** — `stackless up --lease 2h …` for throwaway agents.
3. **Run `stackless list --json`** periodically; tombstoned instances still
   appear with context; active instances show remaining lease.
4. Use local workloads for inner loops. Managed integrations can still create
   billable resources when workloads run locally.
5. **Branch on `error.code`** — e.g. `render.payment.not_confirmed` means rerun
   with `--confirm-paid`, not retry blindly.

Spend summaries after cloud `up`/`down` are bounded by Stripe Project hard
caps configured in `[stack.projects.stripe]`.

## MCP integration

Agents can drive stackless through the hidden stdio MCP server (no subprocess
shell parsing):

```json
{
  "command": "stackless",
  "args": ["mcp"]
}
```

Tools mirror CLI verbs with `--json` forced: `stackless_check`,
`stackless_doctor`, `stackless_up`, `stackless_down`, `stackless_verify`,
`stackless_status`, `stackless_list`, `stackless_logs`. Tool results return the
same JSON envelopes as the CLI on stdout; `stackless_up` also includes NDJSON
progress from stderr in the result text.

See the [stackless agent skill](../.cursor/skills/stackless/SKILL.md) for
error-code branching and lifecycle ordering.
