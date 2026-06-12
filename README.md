# stackless

**Disposable software stacks: named, leased, isolated, proven,
accounted for, destroyed.**

stackless is a CLI that owns the complete lifecycle of disposable
stacks. One declarative file describes your product once — every
service, datastore, secret, and health contract. One verb spawns a
full, isolated, working copy with a name and a URL; one verb proves it
works; one verb (or an expired lease) destroys it verifiably. On a
laptop or on a cloud provider, for a human, a CI job, or — first and
foremost — an AI agent.

```console
$ stackless up demo --on local
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
  datastores, hosted integrations, secrets, wiring, and health
  contracts once. Wiring is interpolation —
  `DATABASE_URL = "${datastores.db.url}"`,
  `CLERK_SECRET_KEY = "${integrations.clerk.secret_key}"` — and the
  startup order is *derived* from it; there is no `depends_on` to
  drift.
- **Hosted integrations** are provisioned as stack resources too. For
  Clerk, Stackless creates the app through Stripe Projects, can enable
  slugged Organizations, and exposes the selected publishable/secret
  keys for services and verify.
- **An instance** is a named, short-lived incarnation of the stack.
  The name is assigned at birth, validated DNS-safe, and everything
  the instance owns derives from it. Any number of instances coexist
  without colliding on ports, names, data, or credentials.
- **Substrates** decide where instances live. Pass `--on local` or
  `--on render` at creation (required); resume uses the recorded
  substrate and never asks again:
  - **local** — services run as host processes from your declared
    commands; datastores run as labeled containers with per-instance
    volumes; everything meets at a built-in reverse proxy, so origins
    are derivable from the name alone:
    `http://{service}.{instance}.localhost:4444`.
  - **render** — the same definition deploys to
    [Render](https://render.com) through the same Stripe Project used
    for hosted integrations (one long-lived project per stack, one
    named environment per instance), with hard spend caps and
    per-invocation paid consent (`--confirm-paid`).
- **Sources are git references** (`repo` + `ref`), materialized per
  instance from a shared object cache. For the edit loop,
  `--source service=/path/to/checkout` pins a service to your dirty
  worktree — explicit, recorded, local-only.
- **Health gates `up`** (invariant: provisioned ≠ configured ≠
  verified). An instance is not "up" because processes started; it is
  up when every service's health contract passes through its public
  origin. `stackless verify` runs the stack's own proof command (the
  smoke tier) with the instance's origins and env exported.
- **Every instance carries a lease** (local default 24h, render 8h).
  Mutating verbs and successful `verify` renew it; when it expires, a
  reaper sends the instance through the same verified teardown as
  `down`. Teardown refuses to report success while anything that bills
  or holds state survives — and leaves a tombstone, so `status` and
  `logs` still answer *why* an instance disappeared.

## Verbs

| Verb | Does |
|---|---|
| `up <name>` | Create **or resume** an instance (no separate resume verb). `--on <substrate>` **required at creation**, `--file <path>`, `--source svc=path`, `--lease 8h`, `--confirm-paid` |
| `down <name>` | Verified teardown; exits non-zero listing survivors if anything remains |
| `verify <name>` | Run the stack's proof contract; renews the lease |
| `status <name>` | Staged truth per service: provisioned → prepared → started → healthy, downgraded by observation |
| `list` | All instances with substrate, stage, remaining lease |
| `logs <name> [service]` | Captured service output (local files / Render log API), survives teardown |
| `check <file>` | Parse + validate a definition, print the derived graph; `--on <substrate>` adds substrate checks |

Every command is non-interactive, supports `--json`, and exits with
codes an agent can branch on. Every error carries three parts: what
failed, why (observed, not guessed), and how to proceed:

```json
{
  "ok": false,
  "error": {
    "code": "state.lock.held",
    "message": "instance \"demo\" is locked by operation \"up\" (pid 4242, ...)",
    "remediation": "wait for the running operation on \"demo\" to finish and retry; ..."
  }
}
```

Codes are stable, versioned API surface — branch on `error.code`,
never on prose.

## Quick start

```console
$ cargo build --release            # one binary: target/release/stackless
$ cd your-repo                     # containing a stackless.toml
$ stackless check stackless.toml   # validate + see the derived graph
$ stackless up demo --on local                # clone, build, wire, health-gate
$ stackless down demo              # verified teardown
```

Writing a definition: start from [docs/SCHEMA.md](docs/SCHEMA.md) —
it is written to be sufficient on its own, for humans and agents.

## Workspace layout

| Crate | Owns |
|---|---|
| `stackless-core` | Definition model + validation + interpolation + derived graph, the SQL state store (instances, leases, locks, checkpoint journal), the lifecycle engine, the `Substrate` trait |
| `stackless-local` | Local substrate: process spawn/teardown, container datastores, gix source materialization, wiring |
| `stackless-render` | Render substrate, hosted integration provisioning through Stripe Projects, and Render REST client |
| `stackless-daemon` | The one resident component: reverse proxy, supervision, lease reaper, launchd/systemd persistence |
| `stackless` | The clap CLI binary (also hosts the daemon via `daemon run`) |

Substrates are plugins behind one trait: adding a provider crate
requires no changes to the engine or state machinery — only a registry
entry in the binary.

## Status

v0 lifecycle layer, under active development; the local substrate and
lifecycle machinery (M1–M7) are implemented and verified, the Render
substrate is implemented with its live verification in progress, and
the opt-in fleet plane (shared state store across machines) is in
flight. The secret-blind egress boundary described in VISION.md is
deliberately sequenced after v0 — see ARCHITECTURE.md §0 for the v0
secrets posture (operator-visible, test-scoped).
