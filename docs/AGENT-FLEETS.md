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
path, its own `definition_dir`, and its own Stripe Projects lock domain. Name
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

## Fleet state plane: `STACKLESS_STATE_URL`

By default each machine keeps state in a local SQLite file under the XDG state
dir. Instance names are unique on that machine; leases and operation locks are
local.

**Opt-in fleet mode** points every stackless process at a shared Turso Cloud
database:

```bash
export STACKLESS_STATE_URL="libsql://your-db.turso.io"
export STACKLESS_STATE_TOKEN="your-token"
```

Effects:

- **Name uniqueness is fleet-wide** — the `UNIQUE` constraint on instance names
  applies across all operators sharing the URL.
- **Leases and locks are CAS on the primary** — compare-and-swap
  `UPDATE`s replace PID-local assumptions; the reaper and mutating verbs
  coordinate across machines.
- **No cloud account required for solo local use** — unset the variables to
  return to the default file backend.

Verify connectivity before a fleet run:

```bash
stackless doctor --json
stackless list --json
```

If state open fails, the error code is `state.store.open_failed`; remediation
mentions `STACKLESS_STATE_URL` and `STACKLESS_STATE_TOKEN`. Turso Cloud live
verification is tracked in the project roadmap; the seam is implemented in
`stackless-core` (`Store::open_configured`).

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
sharing the state plane — `state.instance.exists` is global in fleet mode.

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
4. **Use local substrate for inner loops** — reserve cloud for integration
   smokes; `--on local` avoids catalog spend entirely.
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
