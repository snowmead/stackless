<p align="center">
  <img src="docs/assets/banner.png" alt="stackless" width="100%" />
</p>

# stackless

<a href="https://crates.io/crates/stackless"><img src="https://img.shields.io/crates/v/stackless.svg?label=crates.io" alt="crates.io" /></a>
<a href="https://www.npmjs.com/package/stackless-sdk"><img src="https://img.shields.io/npm/v/stackless-sdk.svg?label=npm" alt="npm" /></a>
<a href="https://pypi.org/project/stackless-sdk/"><img src="https://img.shields.io/pypi/v/stackless-sdk.svg?label=PyPI" alt="PyPI" /></a>
<a href="https://pkg.go.dev/github.com/snowmead/stackless/sdks/go"><img src="https://pkg.go.dev/badge/github.com/snowmead/stackless/sdks/go.svg" alt="Go Reference" /></a>
<a href="https://github.com/snowmead/stackless/releases/latest"><img src="https://img.shields.io/github/v/release/snowmead/stackless?label=release" alt="GitHub release" /></a>
<a href="https://github.com/snowmead/stackless/actions/workflows/ci.yml"><img src="https://github.com/snowmead/stackless/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
<a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" alt="License: MIT OR Apache-2.0" /></a>

**Ephemeral software stacks: named, leased, isolated, proven, destroyed.**

## What

stackless is a CLI that owns the full lifecycle of an ephemeral stack.
One `stackless.toml` describes the product — services, secrets, wiring,
health. One verb (`up`) spawns an isolated, named instance with a URL; one
verb (`verify`) proves it; one verb (`down`) or an expired lease destroys
it verifiably.

Unopinionated about the application. Opinionated about the lifecycle.

Built for **AI agents first**. A human at a terminal is a guest in an
interface shaped for machines — do not drive stacks by hand.

## Why

Agent fleets need many simultaneous, isolated, ephemeral instances per
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

A `stackless.toml` describes the complete stack:

```toml
[stack]
name = "demo"

[integrations.clerk]
provider = "clerk"
app_name = "${stack.name}-${instance.name}"

[integrations.neon]
provider = "neon"

[services.web]
source = { repo = "https://github.com/you/app", ref = "main" }
root_origin = true
health = { path = "/", contains = "ok" }
env = { CLERK_SECRET_KEY = "${integrations.clerk.secret_key}", DATABASE_URL = "${integrations.neon.database_url}" }

  [services.web.vercel]
  framework = "vite"
  build = "npm run build"

[services.db]
source = { repo = "https://github.com/you/app", ref = "main" }
health = { path = "/health", contains = "ready" }

  [services.db.local]
  run = "docker run --rm -p $PORT:5432 postgres:16"
```

### Install

Binary:

```console
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh
```

Re-running the installer upgrades in place. When installed via that shell
installer (install receipt present), the CLI also auto-updates from GitHub
Releases on ordinary use and via `stackless update`. Set
`STACKLESS_NO_SELF_UPDATE=1` to disable.

Agent skill:

```console
bunx skills add snowmead/stackless --skill stackless -g
```

### Lifecycle

```bash
stackless check stackless.toml --on local --json
stackless up --name demo --on local --json
stackless verify demo --json
stackless down demo --json
```

- `--on <substrate>` is required at creation. Supported hosts and
catalog integrations: [PROVIDERS.md](PROVIDERS.md). Resume by name;
substrate is fixed after create.
- Cloud needs provider API keys (see `stackless doctor`); paid resources
need `--confirm-paid`.
- Local edit loop: `--source svc=/path` pins a service to a checkout
(cloud rejects `--source`).
- Integrations via `[integrations.*]` provision through Stripe Projects
(every provider in the catalog registry is first-class). Authoring:
`init` / `adopt`, then `check`. Full `stackless.toml` reference:
[docs/SCHEMA.md](docs/SCHEMA.md).

### Machine contract

- **stdout** — final envelope: `{ "ok": true, … }` or
`{ "ok": false, "error": { … } }`.
- **stderr** — NDJSON progress events during `up --json`.
- Every error carries what failed, why (observed), and remediation.
Branch on `error.code` only.

Fleets, parallel agents, and MCP:
[docs/AGENT-FLEETS.md](docs/AGENT-FLEETS.md).

### Verbs


| Verb                        | Does                                                     |
| --------------------------- | -------------------------------------------------------- |
| `up [--name]`               | Create or resume; `--on` required at creation            |
| `down <name>`               | Verified teardown                                        |
| `verify <name>`             | Run proof contract; renews lease                         |
| `status` / `list`           | Staged truth / all instances                             |
| `logs <name>`               | Captured output (survives teardown)                      |
| `check <file>`              | Validate definition + derived graph                      |
| `bind`                      | Compile `stackless.toml` → IDL + typed language bindings |
| `init` / `adopt` / `doctor` | Scaffold, detect, preflight                              |


Every command is non-interactive and exits with codes an agent can branch on.

### Typed bindings

`stackless bind` projects a stack definition into a language-neutral IDL
(`.stackless/stack.idl.json`) and typed bags so tests can name services,
verify tiers, and integration outputs without stringly DNS keys. Emitters
cover Rust, TypeScript, Go, and Python. Each language gets `Origins` /
`bindOrigins`, `Integrations` / `bindIntegrations`, `SECRETS_REQUIRED`, and
`VerifyTier` when declared. Language identifiers are computed at emit time
from DNS wire names and provider output keys (not stored as language idents
in the IDL).

```bash
stackless bind --file stackless.toml \
  --idl .stackless/stack.idl.json \
  --emit typescript=e2e/stack.gen.ts \
  --emit rust=tests/support/stack_bind.rs \
  --emit go=internal/stack/origins.go \
  --emit python=tests/stack_bind.py

# Aliases still work: --ts PATH, --rs PATH
# Go package defaults to stacklessbind; override with --go-package NAME

# CI: fail if any output is stale
stackless bind --file stackless.toml \
  --idl .stackless/stack.idl.json \
  --emit typescript=e2e/stack.gen.ts \
  --emit rust=tests/support/stack_bind.rs \
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

### Language SDKs

Published packages for Rust, TypeScript, Python, and Go — same lifecycle
verbs (`up` / `verify` / `down` / …), same envelopes. Versioned in lockstep
with the CLI; publish runbook: [docs/PUBLISHING.md](docs/PUBLISHING.md).


| Language   | Package                                                             | Source                               |
| ---------- | ------------------------------------------------------------------- | ------------------------------------ |
| Rust       | [stackless](https://crates.io/crates/stackless) (crates.io)         | [crates/stackless](crates/stackless) |
| TypeScript | [stackless-sdk](https://www.npmjs.com/package/stackless-sdk) (npm)  | [sdks/typescript](sdks/typescript)   |
| Python     | [stackless-sdk](https://pypi.org/project/stackless-sdk/) (PyPI)     | [sdks/python](sdks/python)           |
| Go         | [sdks/go](https://pkg.go.dev/github.com/snowmead/stackless/sdks/go) | [sdks/go](sdks/go)                   |


```toml
# Rust
[dependencies]
stackless = "0.2"
```

```bash
# TypeScript
npm i stackless-sdk

# Python (import stackless)
pip install stackless-sdk

# Go
go get github.com/snowmead/stackless/sdks/go@v0.3.3
```

All clients need the `stackless` CLI on `PATH` (or `STACKLESS_BIN`) for the
operator daemon. Rust can also embed a hermetic daemon via feature
`test-support` / `TestContext`. Non-Rust packages speak the CLI JSON
protocol ([sdks/PROTOCOL.md](sdks/PROTOCOL.md)).

```rust
use stackless::{Client, Create, UpRequest};

let client = Client::system()?;
let created = client.up(UpRequest::Create(
    Create::new("stackless.toml", "local").named("demo"),
))?;
println!("{}", created.origin("web")?);
client.verify(&created.name, None)?;
client.down(&created.name)?;
```

```typescript
import { Client } from "stackless-sdk";

const client = Client.system();
const up = await client.up({
  kind: "create",
  name: "demo",
  on: "local",
  file: "stackless.toml",
});
console.log(up.origins.web);
await client.verify(up.instance);
await client.down(up.instance);
```

`up` returns service origins and, when present, nested integration outputs.
Prefer verify-tier env interpolation when secrets must not appear on stdout.
Product test harnesses (Playwright fixtures, etc.) belong in the application
repo; stackless stops at Client + bind + delivery.

## Development

Activate mise tools (`mise install`, then `mise exec --` or an activated
shell). Gates live in `mise.toml` `[tasks]`:


| Task             | Does                                           |
| ---------------- | ---------------------------------------------- |
| `mise run check` | fmt + clippy + taplo                           |
| `mise run test`  | `cargo nextest run --workspace --all-features` |
| `mise run ci`    | check + test + supply-chain audit/deny/vet     |


Plain `cargo build` / `cargo test` also work. Architecture:
[ARCHITECTURE.md](ARCHITECTURE.md). Supported providers:
[PROVIDERS.md](PROVIDERS.md). Contributor map and provider tooling:
[CLAUDE.md](CLAUDE.md). Releases: [CHANGELOG.md](CHANGELOG.md). Cursor Cloud
notes: [AGENTS.md](AGENTS.md).

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  https://opensource.org/licenses/MIT)

at your option.
