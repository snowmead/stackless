# stackless

**Disposable software stacks: named, leased, isolated, proven,
accounted for, destroyed.**

stackless is a CLI that owns the complete lifecycle of disposable
stacks. One declarative file describes your product once — every
service, secret, and health contract. One verb spawns a
full, isolated, working copy with a name and a URL; one verb proves it
works; one verb (or an expired lease) destroys it verifiably. On a
laptop or on a cloud provider, for a human, a CI job, or — first and
foremost — an AI agent.

```console
$ stackless up --name demo --on local
demo: up on local (all health contracts passed)
  api: http://api.demo.localhost:4444
  web: http://demo.localhost:4444

$ stackless verify demo
demo: verify passed (lease renewed)

$ stackless down demo
demo: destroyed, verified gone; tombstone and logs kept
```

Unopinionated about the application. Opinionated about the lifecycle.
See [VISION.md](VISION.md) for why this exists and the invariants it
refuses to break; [ARCHITECTURE.md](ARCHITECTURE.md) for how it is
built; [docs/SCHEMA.md](docs/SCHEMA.md) for the complete
`stackless.toml` reference.

## The stranger test

A stranger — or an agent handed nothing but a repo containing a
`stackless.toml` — runs one command and gets a working, isolated, named
copy of the entire product with a URL they can open. One more command
proves it healthy. They walk away; within the lease window it is gone,
verifiably. No wiki page, no teammate, no manual cleanup.

## How it works

- **A stack definition** (`stackless.toml`) declares services,
  hosted integrations, secrets, wiring, and health contracts once.
  Wiring is interpolation — e.g.
  `CLERK_SECRET_KEY = "${integrations.clerk.secret_key}"` — and the
  startup order is *derived* from it; there is no `depends_on` to drift.
- **Hosted integrations** (`[integrations.<name>]`, with a required
  `provider` naming the catalog adapter) are provisioned as stack
  resources too. For Clerk (`provider = "clerk"`), Stackless creates
  the app through Stripe Projects, can enable slugged Organizations,
  and exposes the selected publishable/secret keys for services and
  verify.
- **Secrets** — `[secrets].required` keys resolve from `.stackless.env`
  beside the definition file (vault pull layers in when
  `[stack.projects.stripe].project` is recorded). A required key
  missing from every source fails validation before anything
  provisions.
- **An instance** is a named, short-lived incarnation of the stack.
  Pass `--name` at creation (DNS-safe); omit it and stackless assigns
  `{stack.name}-{uuid}`. Everything the instance owns derives from
  the name. Any number of instances coexist without colliding on ports,
  names, data, or credentials.
- **Substrates** (stack hosts) decide where instances live. Pass
  `--on local`, `--on render`, `--on vercel`, `--on fly`, or `--on netlify`
  at creation (required); resume uses the recorded substrate and never asks again:
  - **local** — services run as host processes from your declared
    commands; everything meets at a built-in reverse proxy, so origins
    are derivable from the name alone:
    `http://{service}.{instance}.localhost:4444`.
  - **render** — the same definition deploys to
    [Render](https://render.com) through the same Stripe Project used
    for hosted integrations (one long-lived project per stack, one
    named environment per instance), with hard spend caps and
    per-invocation paid consent (`--confirm-paid`). Stripe Projects
    provisions catalog resources; the Render REST API handles env vars,
    deploys, health waits, and teardown verification (`RENDER_API_KEY`
    or `.render-api-key`). After cloud `up`/`down`, a spend summary is
    printed (bounded by the project hard cap).
  - **vercel** — git-backed projects on
    [Vercel](https://vercel.com) via Stripe `vercel/project` (and
    optional `vercel/pro` when `[stack.vercel].plan = "pro"`). Stripe
    creates/links the project; the Vercel REST API pushes interpolated
    env, triggers git deployments, polls until READY, and verifies
    teardown (`VERCEL_TOKEN` or `.vercel-token`). `source.repo` must be a
    public GitHub HTTPS remote.
  - **fly** — container apps on [Fly.io](https://fly.io) via Stripe
    `flyio/app` (paid → `--confirm-paid`). Stripe creates the app and hands
    back a scoped deploy token; the Fly Machines REST API uses it to
    allocate the app's public IPs, run the service's prebuilt `image` as a
    machine, and poll it to `started`, health-gating on
    `https://{stack}-{instance}-{service}.fly.dev`. Teardown removes the
    Stripe resource and confirms via its registration (no operator API
    token needed). v0 is image-only (no build-from-source).
  - **netlify** — static sites on [Netlify](https://netlify.com) via Stripe
    `netlify/project` (free). Stripe creates the site and returns a scoped
    token; the substrate clones the pinned ref and runs the Netlify
    file-digest deploy (SHA1 per file, upload only what's missing), polls to
    `ready`, and health-gates on
    `https://{stack}-{instance}-{service}.netlify.app`. Teardown removes the
    Stripe resource and confirms via its registration. v0 is static-upload
    (no build step).
- **Sources are git references** (`repo` + `ref`), materialized per
  instance from a shared object cache. For the edit loop,
  `--source service=/path/to/checkout` pins a service to your working
  copy in place (single active instance per checkout). Add `--dirty` to
  snapshot each pin's uncommitted tree into instance-owned space instead
  — explicit, recorded, local-only, parallel-instance safe.
- **`setup` / `prepare` hooks** — optional per service. `setup` runs
  once after source materialization (toolchain, deps); `prepare` runs on
  every `up` after the service's dependencies are ready and before it
  starts (migrations, seed).
- **Health gates `up`** (invariant: provisioned ≠ configured ≠
  verified). An instance is not "up" because processes started; it is
  up when every service's health contract passes through its public
  origin. `stackless verify` runs the stack's own proof command (the
  smoke tier) with the instance's origins and env exported.
- **Every instance carries a lease** (local default 24h; render,
  vercel, fly, and netlify default 8h).
  Mutating verbs and successful `verify` renew it; when it expires, a
  reaper sends the instance through the same verified teardown as
  `down`. Teardown refuses to report success while anything that bills
  or holds state survives — and leaves a tombstone, so `status` and
  `logs` still answer *why* an instance disappeared.

## Verbs

| Verb | Does |
|---|---|
| `up [--name <name>]` | Create **or resume** an instance (no separate resume verb). `--name` optional at creation (`{stack}-{uuid}`); `--on <substrate>` **required at creation**; `--file <path>`, `--source svc=path`, `--dirty` (requires `--source`), `--lease 8h`, `--confirm-paid` |
| `down <name>` | Verified teardown; exits non-zero listing survivors if anything remains |
| `verify <name> [--tier <name>]` | Run the stack's proof contract; renews the lease |
| `status <name>` | Staged truth per service: provisioned → prepared → started → healthy, downgraded by observation |
| `list` | All instances with substrate, `active`/`tombstoned`, per-service stage, remaining lease |
| `logs <name> [service]` | Captured service output (local files, Render/Vercel/Netlify/Fly APIs where wired); survives teardown; `--tail` (default 100) |
| `check <file>` | Parse + validate a definition, print the derived graph; `--on <substrate>` adds substrate checks |

Every command is non-interactive, supports `--json`, and exits with
codes an agent can branch on.

### Agent output (`--json`)

- **stdout** — final success or failure envelopes (`ok: true/false`).
- **stderr** — human prose in non-JSON mode; in `--json` mode, **NDJSON
  progress events** during `up` (so stdout stays machine-parseable).

Every error carries three parts: what failed, why (observed, not
guessed), and how to proceed:

```json
{
  "ok": false,
  "error": {
    "schema_version": 1,
    "code": "state.lock.held",
    "message": "instance \"demo\" is locked by operation \"up\" (pid 4242, ...)",
    "step": "start:api",
    "instance": "demo",
    "remediation": "wait for the running operation on \"demo\" to finish and retry; ...",
    "context": {
      "service": "api",
      "hook": "setup",
      "command": "mise install",
      "log_path": "/path/to/log",
      "log_tail": "last lines of captured output on hook/health failures"
    }
  }
}
```

`step`, `instance`, and `context` fields are omitted when not
applicable; `context` subfields are populated only when observables
exist.

During `up --json`, stderr emits one NDJSON object per plan step:
`step_started`, `step_skipped`, `step_completed`, or `step_failed`,
with `schema_version`, `instance`, `step`, `kind`, `node`, `index`,
`total`, optional `code`, `at_epoch_ms`, and optional `duration_ms`.

Every success verb on stdout includes `{ "schema_version": 1, "ok": true, … }`:
`up` (with `executed`, `skipped`, `duration_ms`, `steps`, `origins`, optional
`spend`), `down` (`outcome`, optional `spend`), `verify` (`duration_ms`,
`exit_status`, `log_path`, optional `tier`), `status`, `list`, `logs`,
`check`, `init`, `adopt`, and `doctor`. Cloud `up`/`down` may include a
structured `spend` object (`provider`, `cap_usd`, `summary`, optional `data`).

`status`/`list --json` may include `persistence_warning` when daemon boot
persistence failed (leases then depend on the daemon staying up).

Codes are stable, versioned API surface — branch on `error.code`,
never on prose.

For parallel agents, shared fleet state, and MCP wiring, see
[docs/AGENT-FLEETS.md](docs/AGENT-FLEETS.md).

## Quick start

### Install

Prebuilt binaries (macOS and Linux) ship on GitHub Releases via
[cargo-dist](https://github.com/axodotdev/cargo-dist). Release notes come from
[CHANGELOG.md](CHANGELOG.md).

```console
$ curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh
$ stackless --version
stackless 0.1.4
```

The installer places `stackless` on your `PATH` (under `$CARGO_HOME/bin` by
default). Override the download host with `STACKLESS_INSTALLER_GITHUB_BASE_URL`
or pin a direct artifact URL with `STACKLESS_DOWNLOAD_URL` (see the generated
installer script).

### Build from source

```console
$ cargo build --release            # one binary: target/release/stackless
$ cd your-repo                     # containing a stackless.toml
$ stackless check stackless.toml   # validate + see the derived graph
$ stackless up --name demo --on local           # clone, build, wire, health-gate
$ stackless down demo              # verified teardown
```

Local substrate: app services run as host processes.

Writing a definition: start from [docs/SCHEMA.md](docs/SCHEMA.md) —
it is written to be sufficient on its own, for humans and agents.

## Development

The repository pins its toolchain and auxiliary tools via [mise](https://mise.jdx.dev/):

```console
# one-time: install mise (https://mise.jdx.dev/getting-started.html), then:
mise install
```

This provides the exact Rust 1.96.0 (via `rust-toolchain.toml` + mise) plus `cargo-nextest`, `cargo-audit`, `cargo-deny`, `cargo-vet`, `cargo-dist`, and `taplo`.

Common commands (also wired as `mise run <task>`):

- Tests: `cargo nextest run --workspace` (or `mise run test`)
- Hygiene ("cargo crap"): `mise run ci` (fmt + clippy + taplo + nextest + audit + deny + vet)
- Individual: `cargo audit`, `cargo deny check`, `cargo vet`, `taplo fmt --check`
- Live smoke tests against real providers: `mise run smoke-vercel` / `mise run smoke-render` / `mise run smoke-fly` / `mise run smoke-netlify` / `mise run smoke` (creds from `.stackless.env`) — see [docs/SELFTEST.md](docs/SELFTEST.md)

Releases use `cargo-dist` (see generated `.github/workflows/release.yml`).

The original `cargo build` / `cargo test` paths remain valid.

## Workspace layout

| Crate | Owns |
|---|---|
| `stackless-core` | Definition model + validation + interpolation + derived graph, the SQL state store (local `rusqlite` file; opt-in fleet plane via `libsql` remote), instances, leases, locks, checkpoint journal, the lifecycle engine, the `Substrate` trait |
| `stackless-stripe-projects` | Neutral Stripe Projects CLI driver: project anchor (`[stack.projects.stripe]`), per-instance environments, catalog add/remove, env materialization |
| `stackless-integrations` | Hosted integration routing and provider adapters (Clerk today); substrates call here for provision / observe / destroy |
| `stackless-local` | Local substrate: process spawn/teardown, source materialization (via `stackless-git`), wiring, hosted integrations |
| `stackless-git` | Pure-Rust git (backed by `grit-lib`): one bare cache repo per source URL with thin per-instance checkouts sharing objects via `alternates` (local materialization); shallow clone + checkout for cloud prepare |
| `stackless-render` | Render substrate (REST calls go through the generated `render-client` crate) |
| `stackless-vercel` | Vercel substrate (REST calls go through the generated `vercel-client` crate) |
| `render-client` / `vercel-client` | Provider REST API clients generated by `cargo-progenitor` from the vendored OpenAPI specs (regenerate via `specs/regen-clients.sh`) |
| `stackless-daemon` | The one resident component: reverse proxy, supervision, lease reaper, launchd/systemd persistence |
| `stackless` | The clap CLI binary (also hosts the daemon via `daemon run`) |

Substrates are plugins behind one trait: adding a provider crate
requires no changes to the engine or state machinery — only a registry
entry in the binary.

## Providers

Stripe Projects is the internal catalog driver — never declared in
`stackless.toml`. Checked items work today; unchecked items are not
implemented yet.

### Stack hosts (`stackless up --on`)

- [x] local
- [x] render
- [x] vercel
- [x] fly.io
- [ ] railway
- [x] netlify
- [ ] cloudflare workers
- [ ] gitlab
- [ ] laravel cloud
- [ ] wordpress.com

### Integrations (`[integrations.*]` / `provider`)

- [x] clerk
- [ ] auth0
- [ ] workos
- [ ] privy
- [ ] supabase

### Platform

- [x] `stackless logs` (local)
- [x] `stackless logs` (render)
- [x] `stackless logs` (vercel — build events)
- [x] `stackless logs` (netlify — deploy metadata)
- [x] `stackless logs` (fly — machine events)
- [x] fleet state plane (libsql; live Turso verification via `mise run smoke-fleet`)

## Limitations

- **Cloud lease reaping runs from the operator machine.** The reaper lives in
  the local daemon. If the machine sleeps past a cloud instance's lease, the
  instance outlives its lease until the machine wakes; hard spend caps bound
  the leakage.
- **Trust boundary is v0 posture.** Secrets are operator-visible in
  `.stackless.env`; default-deny egress and secret blinding are sequenced after
  the lifecycle layer (see ARCHITECTURE.md §0).

## Status

v0 lifecycle layer, under active development. Local substrate, daemon,
and lifecycle engine are implemented and tested. Render and Vercel
substrates are implemented (Stripe Projects provisions catalog
resources; each cloud host's REST API handles post-provision lifecycle
steps). Live end-to-end verification on real cloud accounts is ongoing.
Opt-in fleet mode shares state across machines; Turso Cloud live
verification is pending. The secret-blind egress boundary described in
VISION.md is deliberately sequenced after v0 — see ARCHITECTURE.md §0
for the v0 secrets posture (operator-visible, test-scoped).