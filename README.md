# stackless

**Disposable software stacks: named, leased, isolated, proven,
accounted for, destroyed.**

## What

stackless is a CLI that owns the full lifecycle of a disposable stack.
One `stackless.toml` describes the product — services, secrets, wiring,
health. One verb (`up`) spawns an isolated, named copy with a URL; one
verb (`verify`) proves it; one verb (`down`) or an expired lease destroys
it verifiably.

Unopinionated about the application. Opinionated about the lifecycle.

Built for **AI agents first**. A human at a terminal is a guest in an
interface shaped for machines — do not drive stacks by hand.

## Why

Agent fleets need many simultaneous, isolated, abandonable stacks per
day. Container tools, IaC, and provider CLIs each own a layer and none
of the whole — so every team rebuilds naming, wiring, teardown, and cost
hygiene, and rediscovers the same failure modes.

stackless is that glue: the lifecycle layer between an agent and the
stack it works on. An agent handed a repo with `stackless.toml` runs
`up`, gets a working named URL, proves health, walks away; within the
lease window it is gone, verifiably. No wiki, no teammate, no manual
cleanup.

Invariants and the trust boundary: [VISION.md](VISION.md).

## How

### For agents

1. Install the binary (below), or build from this repo.
2. Point the agent at [`.cursor/skills/stackless/SKILL.md`](.cursor/skills/stackless/SKILL.md)
   and [docs/SCHEMA.md](docs/SCHEMA.md).
3. Always pass `--json`. Branch on `error.code`, never on prose.

Humans: install once, then hand the skill (or this repo) to an agent.

### Install

```console
$ curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh
$ stackless --version
stackless 0.1.6
```

From source: `cargo build --release` → `target/release/stackless`.

### Lifecycle

```bash
stackless check stackless.toml --on local --json
stackless up --name demo --on local --json
stackless verify demo --json
stackless down demo --json
```

- `--on <substrate>` is required at creation (`local`, `render`, `vercel`,
  `fly`, `netlify`). Resume by name; substrate is fixed after create.
- Cloud needs provider API keys (see `stackless doctor`); paid resources
  need `--confirm-paid`.
- Local edit loop: `--source svc=/path` pins a service to a checkout
  (cloud rejects `--source`).
- Integrations today: Clerk via `[integrations.*]`. Authoring:
  `init` / `adopt`, then `check`.

### Machine contract

- **stdout** — final envelope: `{ "ok": true, … }` or
  `{ "ok": false, "error": { … } }`.
- **stderr** — NDJSON progress events during `up --json`.
- Every error carries what failed, why (observed), and remediation.
  Branch on `error.code` only.

Fleets, parallel agents, MCP: [docs/AGENT-FLEETS.md](docs/AGENT-FLEETS.md).
Full agent playbook: [`.cursor/skills/stackless/SKILL.md`](.cursor/skills/stackless/SKILL.md).

### Verbs

| Verb | Does |
|---|---|
| `up [--name]` | Create or resume; `--on` required at creation |
| `down <name>` | Verified teardown |
| `verify <name>` | Run proof contract; renews lease |
| `status` / `list` | Staged truth / all instances |
| `logs <name>` | Captured output (survives teardown) |
| `check <file>` | Validate definition + derived graph |
| `bind` | Compile `stackless.toml` → IDL + typed TS/Rust bindings |
| `init` / `adopt` / `doctor` | Scaffold, detect, preflight |

Every command is non-interactive and exits with codes an agent can branch on.

### Typed bindings

`stackless bind` projects a stack definition into a language-neutral IDL
(`.stackless/stack.idl.json`) and optional TypeScript / Rust origins bags so
tests can name services and verify tiers without stringly DNS keys.

```bash
stackless bind --file stackless.toml \
  --idl .stackless/stack.idl.json \
  --ts e2e/stack.gen.ts \
  --rs tests/support/stack_bind.rs

# CI: fail if any output is stale
stackless bind --file stackless.toml \
  --idl .stackless/stack.idl.json \
  --ts e2e/stack.gen.ts \
  --rs tests/support/stack_bind.rs \
  --check
```

Rust `build.rs` consumers that already check in the IDL can regenerate only
`$OUT_DIR` via `stackless-bindgen` (no `stackless-core` / libsql link):

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    stackless_bindgen::emit_rust(".stackless/stack.idl.json")?;
    Ok(())
}
```

### Rust SDK

The `stackless` crate is both the CLI and a sync library. Depend on it to
bring environments up and down from Rust (integration tests, CI tooling,
or operators that prefer an in-process API over shelling out). Linking
the crate does **not** require your binary to implement CLI subcommands.

Two modes:

| Mode | API | Purpose |
| --- | --- | --- |
| **Operator / system** | `Client::system()` | Shared user daemon (`$XDG_STATE_HOME/stackless`), same as the CLI. Requires the **stackless CLI** installed (`stackless` on `PATH`, or `STACKLESS_BIN` pointing at it). |
| **Hermetic / embed** | `Client::builder().paths(...)` + feature `test-support` / `TestContext` | Private state dir and embedded daemon for unit/integration tests. |

```toml
[dependencies]
stackless = "0.1"

# Hermetic local tests (embedded daemon + temp state dir):
[dev-dependencies]
stackless = { version = "0.1", features = ["test-support"] }
```

```rust
use stackless::{Client, Create, UpRequest};

// Needs the stackless CLI on PATH (or STACKLESS_BIN). Spawns the CLI as
// the operator daemon — not your test/CI binary.
let client = Client::system()?;
let created = client.up(UpRequest::Create(
    Create::new("stackless.toml", "local").named("demo"),
))?;
println!("{}", created.origin("web")?);
client.verify(&created.name, None)?;
client.down(&created.name)?;
```

Paid cloud substrates use `PaidConsent::confirmed()` (same gate as
`--confirm-paid`). Feature flags select substrates (`local`, `render`,
`cloud`, …); defaults match the full CLI. Publishing notes:
[docs/PUBLISHING.md](docs/PUBLISHING.md).

## Docs

- [VISION.md](VISION.md) — why and invariants
- [ARCHITECTURE.md](ARCHITECTURE.md) — how it is built
- [docs/SCHEMA.md](docs/SCHEMA.md) — `stackless.toml` reference
- [docs/AGENT-FLEETS.md](docs/AGENT-FLEETS.md) — parallel agents and shared state
- [`.cursor/skills/stackless/SKILL.md`](.cursor/skills/stackless/SKILL.md) — agent skill
- [CLAUDE.md](CLAUDE.md) — contributor map (crates, mise, provider tooling)
- [docs/PUBLISHING.md](docs/PUBLISHING.md) — crates.io publish order for the SDK wave
- [CHANGELOG.md](CHANGELOG.md) — releases

## Status

v0 lifecycle, under active development. Substrates: local, Render, Vercel,
Fly, Netlify. Cloud lease reaping runs from the operator daemon — a sleeping
machine defers reap until wake; spend caps bound leakage. Secrets posture
(v0): [ARCHITECTURE.md](ARCHITECTURE.md) §0.
