# stackless — architecture

Companion to [VISION.md](VISION.md). This document is being designed
section by section; sections marked **TBD** have not been discussed yet
and contain no decisions. Nothing here is speculative — if it's written
as decided, it was decided deliberately.

## 0. Posture (v0)

- **Rust core, pluggable provisioning.** One Rust workspace, one
  `stackless` binary. The core owns everything the vision is opinionated
  about: identity, state, lifecycle, wiring, verification, leases,
  teardown. Substrate backends may drive external CLIs where those earn
  their keep — the Stripe Projects CLI is the internal catalog driver
  for cloud provisioning and spend tracking (Render, Vercel, Clerk,
  …). Each cloud substrate also talks to the provider's REST API for
  what Stripe Projects cannot express: interpolated env vars, deploy
  triggers, deploy polling, health waits, and teardown verification.
  Operators never declare "use Stripe Projects" in `stackless.toml` —
  stackless always does when a catalog resource is needed. This split is
  proven by atto-web's `cloud-env.ts` (Render) and mirrored on Vercel.
- **The trust boundary is sequenced, not shipped, in v0.** Default-deny
  egress and secret blinding remain the destination (VISION.md), but v0
  ships the lifecycle layer first. v0 keeps the seam that makes the
  boundary additive later: every resource an instance owns (process,
  container, volume, cloud service) is named and labeled with the
  instance name, so wrapping an instance in its own network later is an
  addition, not a redesign.
- **v0 secrets posture:** secrets flow as env vars, sourced from the
  Stripe Projects vault / pulled env files, visible to the operator,
  protected by being test-scoped credentials (the atto model today).

## 1. Stack definition

Decided:

- **Format: TOML**, in a `stackless.toml`. Rust-native culture, serde,
  comments; the schema is deliberately shallow enough that TOML's
  nesting limits don't bite.
- **A service is substrate-independent identity + wiring + health**;
  how a substrate runs it is nested per substrate
  (`[services.api.local]` with a `run` command,
  `[services.api.render]` with repo/runtime/build/start,
  `[services.web.vercel]` with framework/build settings).
- **Code sources are git references** (`repo` + `ref` per service).
  `up` materializes each service's source into instance-owned space
  (clone/worktree). A per-invocation override pins a service to an
  existing checkout instead — the agent's "run my dirty worktree" loop
  is first-class, but it is an explicit, recorded choice, never ambient
  discovery, and it is **local-only**: cloud substrates deploy committed
  refs, so `up --on render` or `up --on vercel` with `--source` fails
  validation, remediation "commit and push, then pin the ref". Declared
  sources are symmetric with cloud deploys (repo + ref) and pass the
  stranger test: one repo with a `stackless.toml` is enough. On Vercel,
  `source.repo` must be a public GitHub HTTPS remote
  (`https://github.com/org/repo`).
- **Wiring is interpolation, and the dependency graph is derived from
  it.** Env values reference a namespace evaluated per instance per
  substrate: `${instance.name}`, `${services.api.origin}`,
  `${datastores.db.url}`, `${secrets.KEY}`. If service A's env
  references service B or datastore D, that *is* the dependency edge —
  startup order is derived from wiring, never declared separately
  (nothing to drift). Origins are derivable from the instance name
  alone on local and Render (Vercel uses the deployment URL after
  `start`), so mutual references between services
  (api ↔ web CORS) are not cycles.

- **Two per-service lifecycle hooks, both optional.** `setup` runs once
  after the service's source is materialized (toolchain, deps —
  `mise install`, `bun install`). `prepare` runs on every `up`, after
  the service's dependencies are ready and before the service itself
  starts (migrations, seed). `prepare` runs on **every substrate**: on
  cloud substrates it executes on the operator's machine from the
  materialized source, with the instance's env exported (e.g. the
  provisioned database's external connection string), sequenced after
  datastores are provisioned and before services deploy. Stacks whose
  services migrate on boot (atto-server runs sqlx migrations in
  `serve`) simply omit migration from `prepare` — migrate-on-boot is
  the less common pattern, so stackless cannot require it. Both hooks
  are contractually safe to re-run: resume re-runs an interrupted
  `setup` in place (warm caches survive, and `mise install`/`bun
  install` are naturally idempotent), and `prepare` already runs on
  every `up`.
- **Health gates `up`; `verify` proves.** Every service declares a
  `health` check (HTTP path + expected status/content, with retry
  budget); `up` refuses to report success until all pass — invariant 2.
  The stack declares one `verify` command (atto: the smoke tier), run
  by the `verify` verb with the instance's origins/env exported. One
  command in v0; named tiers can be added later without breaking the
  schema.
- **Hosted integrations separate logical name from catalog provider.**
  `[integrations.<name>]` is the interpolation slot
  (`${integrations.<name>.<output>}`); `provider` names the catalog
  adapter (`clerk` → `clerk/auth`, provisioned via Stripe Projects
  internally). Each provider declares **managed** (Clerk — global config
  only, runs on provider cloud) or **host-bound** (future — explicit
  `local`/`render`/`vercel` support, optional per-host overrides).
  Provider-specific config is validated by `stackless-integrations`;
  `${integrations.*}` references are ordering edges in the derived graph.

### Schema reference (the atto dogfood as the example)

```toml
# stackless.toml — lives in any repo (the stranger test: this file is enough)

[stack]
name = "atto"

[stack.render]
region = "oregon"

[stack.projects.stripe]
# Recorded after the first `up` provisions hosted integrations/cloud resources.
project = "project_61UqVKJia4fs..."

[stack.verify]
# The proof contract, run by `stackless verify` (§7).
run = "bun e2e/smoke.ts"
env = { ATTO_STACKLESS = "1", ATTO_E2E_WEB_ORIGIN = "${services.web.origin}", ATTO_E2E_API_ORIGIN = "${services.api.origin}", ATTO_E2E_TENANT_SLUG = "${instance.name}", CLERK_SECRET_KEY = "${integrations.clerk.secret_key}", VITE_CLERK_PUBLISHABLE_KEY = "${integrations.clerk.publishable_key}" }

[secrets]
# v0 posture (§0): operator-visible, test-scoped. Resolved from the stack's
# Stripe Projects vault pull, with a local env-file override for
# hand-managed keys.
required = ["GITHUB_PACKAGES_TOKEN"]

[integrations.clerk]
provider = "clerk"
app_name = "${stack.name}-${instance.name}"
credential_set = "development"
organizations = true

# ── datastores: containers locally, managed services on Render ──

[datastores.db]
engine = "postgres"        # readiness built in per engine (§7), never declared
version = "17"

[datastores.db.render]
plan = "basic-256mb"       # paid → requires --confirm-paid (§4)

# ── services: identity + wiring + health, substrate run config nested ──

[services.api]
source = { repo = "https://github.com/haaku-co/atto-server", ref = "main" }
setup = "mise install"     # once, after materialization
prepare = "just migrate-run && just seed"      # every up, deps ready → before start; uses the Stackless DATABASE_URL
env = { DATABASE_URL = "${datastores.db.url}", CORS_ALLOWED_ORIGINS = "${services.web.origin}", TENANT_SLUG = "${instance.name}", CLERK_SECRET_KEY = "${integrations.clerk.secret_key}", RUST_LOG = "info" }
health = { path = "/health", contains = "ok" }   # status defaults to 200

  [services.api.local]
  run = "cargo run"        # $PORT injected; runs in the materialized source dir

  [services.api.render]
  runtime = "rust"
  build = "cargo build --release"
  start = "./target/release/atto-server"
  env = { SQLX_OFFLINE = "true", RUSTUP_TOOLCHAIN = "1.94.0" }   # overlays the common env

[services.web]
source = { repo = "https://github.com/haaku-co/atto-web", ref = "main" }
setup = "mise install && bun install"
root_origin = true         # also claims http://{instance}.localhost (§3)
env = { VITE_API_ORIGIN = "${services.api.origin}", VITE_CLERK_PUBLISHABLE_KEY = "${integrations.clerk.publishable_key}" }
health = { path = "/", contains = 'id="root"' }   # the SPA shell check, productized

  [services.web.local]
  run = "bunx vite --host 127.0.0.1 --port $PORT --strictPort"

  [services.web.render]
  static = { build = "bun install && bun run build", publish = "./dist", spa_rewrite = true }
  env = { BUN_VERSION = "1.3.11", GITHUB_PACKAGES_TOKEN = "${secrets.GITHUB_PACKAGES_TOKEN}" }
```

### The interpolation namespace

| Reference | Resolves to | Notes |
|---|---|---|
| `${stack.name}` | the stack's declared name | useful for hosted integration names |
| `${instance.name}` | the instance's name | the one identity everything derives from |
| `${services.X.origin}` | substrate-appropriate origin | local: `http://x.{instance}.localhost:<port>`; Render: `https://{stack}-{instance}-x.onrender.com`; Vercel: deployment URL after `start` (best-effort `https://{stack}-{instance}-x.vercel.app` before deploy). Derived from the name alone on local/Render; mutual references (api ↔ web) are not cycles |
| `${datastores.X.url}` | connection string | local: mapped loopback port; Render: internal URL for services, external for `prepare` |
| `${secrets.KEY}` | resolved secret value | for renaming; the `secrets = [...]` list injects same-named vars |
| `${integrations.clerk.secret_key}` | Clerk secret key | selected from Stripe Projects' Clerk environments JSON (`CLERK_AUTH_ENVIRONMENTS` or `CLERK_ENVIRONMENTS`) |
| `${integrations.clerk.publishable_key}` | Clerk publishable key | selected from Stripe Projects' Clerk environments JSON (`CLERK_AUTH_ENVIRONMENTS` or `CLERK_ENVIRONMENTS`) |
| `$PORT` | OS-allocated port | injected into local `run` commands only |

Resolution rules: substrate `env` blocks overlay the common `env`; any
reference from service A to service B or datastore D *is* the
dependency edge that orders startup and `prepare`; references to
anything undeclared fail validation at parse time, not at `up` time.

Deliberately absent from the schema: lease duration (a per-invocation
`--lease` flag with substrate defaults, not stack policy), the
dirty-worktree override (per-invocation `--source api=../atto-server`,
local-only, recorded in the manifest, never in the definition),
`image:` runners and third-party egress declarations (both reserved
seams for later phases), and any `depends_on` key — a dependency must
be expressed in wiring, and an ordering need with no wiring expression
is a definition bug. `depends_on` could be added compatibly later, but
only a real stack proving the need reopens that door; a declared-
separately edge is exactly the drift the derived graph exists to
prevent.

Secrets resolution is pinned: the stack's vault pull is the base, and
a gitignored env file next to `stackless.toml` overlays it — the
override wins (the Clerk lesson's hand-managed keys). A `required` key
that resolves from neither fails validation before anything
provisions, naming the sources consulted.

Known soft spot, to revisit before implementation freezes it:
`runtime = "rust"` under a `render` block passes Render's own
runtime vocabulary through — acceptable inside a substrate-specific
block, but worth saying out loud.

## 2. CLI surface, instance identity & state

Decided (generalized from `cloud-env.ts`'s manifest model and the
vision's invariants; flag anything to veto):

- **Verbs.** `stackless up [--name <name>]`, `down <name>`, `verify
  <name>`, `status <name>`, `list`, `logs <name>` (locally the daemon
  captures service output per instance; on Render it fetches recent
  per-service logs through the Render REST API's logs endpoint — recent
  window, no streaming in v0; Vercel has no `logs` verb in v0 — use the
  dashboard; agents need logs to debug a failed health gate, which bites
  hardest on slow cloud deploys). `up` on an existing
  instance resumes it (invariant 3) — there is no separate resume
  verb. **`--name` is optional at creation** (`{stack.name}-{uuid}`
  when omitted); resume needs only `--name`. **The substrate is chosen
  at creation only** (`--on local|render|vercel`, required on first
  `up` for
  a name), becomes part of the instance's
  identity in the state store, and is never asked for again —
  `verify`/`down`/`status` resolve it by name. Names are unique across
  substrates: creating `demo` on Render while a local `demo` exists is
  an error, not a sibling (uniqueness is scoped to the state store —
  per machine by default, fleet-wide when the shared plane is
  configured). `up --on <s>` fails at validation if any service lacks
  the config that substrate requires. All commands are
  non-interactive, support `--json`, and use exit codes an agent can
  branch on. Anything that spends money requires explicit
  per-invocation consent (`--confirm-paid`, the proven pattern).
- **One operation at a time per instance.** Every mutating verb takes
  a per-instance operation lock before touching anything; a second
  invocation fails fast with a stable code rather than interleaving
  checkpoint writes — agents retry stuck commands, so overlapping
  `up`s are a likely event, not a theoretical one. The lock records
  its holder's PID and process start time, so a crashed holder is
  detected with the same liveness check the daemon uses for service
  processes. The reaper respects the lock (§6): an instance
  mid-operation is never reaped, even past lease expiry.
- **Parallel `up` across different instance names** is supported. Two
  cross-process file locks serialize the shared writers that are not
  per-instance: Stripe Projects CLI invocations keyed by canonical
  `definition_dir` (`.projects/state.json`, `env use`, `env --pull`),
  and bare git cache clone/fetch keyed by source URL. Per-instance
  checkouts (`sources/<instance>/`), processes, proxy routes, and
  containers remain parallel. **Parallel agents should use one git
  worktree per agent** so each has a distinct `definition_dir` and
  Stripe lock; reusing the same checkout with `--source` on multiple
  active instances is refused (`engine.source_override.shared`).
- **Errors are an agent-facing contract, not diagnostics.** The vision
  requires "every error stating what to do next"; agents are the
  primary users, so every error stackless emits carries three parts:
  *what* failed (the step and instance), *why* (the observed cause,
  not a guess), and *how to proceed* (a concrete command, flag, or
  fix). In `--json` mode errors serialize as structured objects with
  `schema_version`, a **stable machine-readable code**, optional `step`
  and `instance`, a `context` object (service, hook, command,
  log_path, log_tail, … — only fields with observables), and
  `remediation` — agents branch on codes, never parse prose (the
  lesson of `cloud-env.ts` branching on the Stripe plugin's
  `JSON_REQUIRES_CONFIRMATION`-style codes). Codes are versioned API
  surface: renaming one is a breaking change. **stdout** carries
  final success/failure envelopes; **stderr** carries NDJSON `up`
  progress (`step_started`/`step_skipped`/`step_completed`/`step_failed`)
  in `--json` mode so stdout stays parseable.
- **Identity.** The instance name is given at creation (`--name`, or
  auto-assigned `{stack.name}-{uuid}` when omitted), validated against
  a DNS-safe pattern (it becomes hostnames and cloud service names —
  same constraint `cloud-env.ts` enforces), and persisted in the
  instance manifest. Nothing is ever re-derived from the working
  directory or other ambient context at runtime (invariant 1 — the
  lesson of `.atto-env`'s guard rails).
- **State: a SQL state store.** Instance records, leases, operation
  locks, and the per-step checkpoint journal `cloud-env.ts` proved
  (every step that creates a resource records it before moving on, so
  an interrupted `up` resumes and an interrupted `down` knows
  everything it must hunt down) live in a Turso database. By default
  it is a local file in the per-user XDG state dir (not inside any
  checkout — instances own their materialized code, so no checkout is
  "the" home); no cloud account is ever required for that. Pointing
  stackless at a Turso Cloud database is the opt-in **fleet plane**:
  multiple operator machines share one state store, name uniqueness
  becomes a `UNIQUE` constraint, and lease/lock claims become
  single-statement compare-and-swap `UPDATE`s — the multi-agent
  conflict case most developers never hit. Crate choice, recorded to
  veto: the `turso` crate was the intended local engine, **revised
  2026-06-11 during implementation** — a spike against turso 0.6.1
  proved it takes an exclusive per-process file lock: a second process
  cannot even open the database while one is connected, idle or not,
  `experimental_multiprocess_wal` included. CLI invocations and the
  daemon must share the state store concurrently by design, so the
  local engine is `rusqlite` (bundled SQLite, WAL, busy timeout)
  behind the same `store.rs` seam — same file format, same SQL, a
  contained substitution; revisit when turso ships real multi-process
  support. Fleet mode is unchanged: it needs
  synchronous CAS against the primary, which today only the `libsql`
  crate's remote mode provides — the `turso` crate's cloud story is
  asynchronous last-push-wins sync, which structurally cannot express
  a lease (and its sync engine carries open correctness bugs:
  turso#6617's LWW mega-issue, #6363, #5640). Verified 2026-06-11: the
  `turso` crate has no synchronous remote mode, no open issue or PR
  proposes one, and Turso's own docs direct over-the-wire access to
  `libsql` — so the split is the design, not a stopgap; revisit only
  if upstream posture changes.
- **Teardown leaves a tombstone, not amnesia.** After a verified
  `down` (or reap), the instance's state rows flip to a tombstone and
  its log files survive until a GC window expires — an agent whose
  instance was reaped overnight can still ask `logs`/`status` why.
  Runtime and billable resources are verifiably gone; records on the
  operator's own disk bill nobody, and the vision's "without residue"
  is about the former.
- **Resume reconciles against observation, not memory** (invariant 4):
  on resume, each recorded step is re-checked against the substrate
  (does the container exist? does the Render service answer?) rather
  than trusted from the manifest alone — the manifest says where to
  look, the substrate says what's true.

## 3. Local substrate

Decided:

- **App services run as host processes** from commands declared in the
  definition. Toolchain provisioning is the repo's business (e.g.
  `mise install` in a setup hook) — stackless stays unopinionated.
- **Datastores run as containers** with per-instance volumes (version
  pinning, clean teardown, no host pollution). The container runner —
  pull image, inject env, map port, mount volume, health-check,
  destroy — exists in v0 for datastores only.
- **One wiring story:** everything meets at `localhost` ports allocated
  per instance. stackless's built-in Rust reverse proxy plays the
  portless role: `{instance}.localhost` (and per-service subdomains)
  route to the allocated ports, so origins are derivable from the
  instance name alone and nothing collides.
- **The schema separates what a service *is* from how a substrate *runs*
  it.** A service is name + wiring + env + health contract; *local* runs
  it via its command, *Render* via repo/runtime config. A container
  `image:` runner for app services can be added later (e.g. when the
  boundary work wants topology control) without breaking any definition
  written today.
- **Teardown is verified locally too, mirroring §4's contract:**
  services get SIGTERM then SIGKILL on their process group, confirmed
  dead by PID + start time; containers and volumes are removed through
  the Docker API and confirmed gone; the proxy route is withdrawn.
  `down` exits non-zero listing survivors if anything remains —
  invariant 4 does not have a cloud-only clause. The state rows then
  flip to the §2 tombstone.

Rationale for host processes over containers-only: the container-build
penalty on macOS (virtiofs I/O on cargo builds) is paid on every
instance an agent cycles, while the fidelity containers would buy
(Linux, release builds) is exactly what the Render substrate exists to
prove — VISION.md invariant 9.

### The daemon (local substrate only)

One small resident component per user hosts everything that must
outlive a CLI invocation: the reverse proxy, instance process
bookkeeping, and the lease reaper. Cloud substrates need none of this —
Render keeps services alive and lease expiry there is enforced by the
same reaper from the operator's machine.

- **Same binary, no separate artifact.** The daemon is the `stackless`
  binary running an internal `daemon run` subcommand. Distribution and
  upgrades are the CLI's story; there is no second thing to install or
  version.
- **Spin-up is on demand.** Commands that need the daemon connect to a
  unix socket at a fixed path in the user's state dir. If nothing
  answers, the CLI spawns the daemon detached (under a lock file, so
  concurrent commands race safely) and waits for the socket. No setup
  step, no "is the daemon running" knowledge required — the portless
  `ensure-proxy` pattern, internalized.
- **Boot persistence.** On first start the daemon registers itself as a
  launchd user agent (macOS) / systemd user unit (Linux) with
  keep-alive, pointing at the stable binary path. This is what makes
  the lease a system guarantee across reboots and crashes. If
  registration is refused, stackless degrades loudly: leases are then
  enforced only when the daemon happens to be running, and `status`
  says so.
- **Instance processes are not the daemon's children.** Service
  processes are spawned in their own sessions and survive a daemon
  restart; the daemon supervises by recorded PID + process start time
  (PID-reuse-safe), with stdout/stderr captured to per-instance log
  files in the state dir — size-capped, rotated with a small number of
  generations kept, so disk is bounded by construction and the tail an
  agent needs to debug a health gate is always in the newest
  generation.
- **Upgrade = restart + re-adopt, and re-adoption is just resume.**
  The socket handshake carries the daemon's version; when a newer CLI
  meets an older daemon, it tells it to drain and exit, and the next
  connection (or launchd) starts the new binary. A starting daemon
  always reconciles manifests against observed reality — the same
  invariant-3/4 machinery `up` uses — re-adopting live processes and
  noting dead ones. Nothing is checkpointed specially for upgrades;
  the manifest is already the truth.
- **v0 supervision policy: observe, don't restart.** A crashed service
  marks the instance unhealthy in `status`; agents re-run `up`
  (resume) to recover. Auto-restart is a later policy knob, not a v0
  behavior.

### Ports, origins, routing

- **The proxy is HTTP-only in v0**, listening on one fixed unprivileged
  port — configurable globally, never per instance, because origins
  must stay derivable from the instance name alone:
  `http://{service}.{instance}.localhost:<port>`. Browsers and modern
  resolvers send `*.localhost` to loopback without /etc/hosts entries.
  TLS mode (:443 + a trusted local CA, one-time sudo) is a later
  opt-in, exactly as portless treats it.
- **One service may declare itself the stack's root origin** and
  additionally claims `http://{instance}.localhost:<port>` (atto: web).
- **Service ports are OS-allocated at `up`** (bind `:0`, record in the
  manifest) and injected as `$PORT` alongside the interpolated env.
  Datastore container ports are mapped the same way;
  `${datastores.db.url}` resolves to the mapped loopback port.
- **The proxy routes on the Host header** from a routing table the
  daemon updates as instances come and go.

## 4. Render substrate

Generalizes `cloud-env.ts` wholesale — every mechanism here is already
proven there.

- **One long-lived Stripe project per stack** holds hosted integrations
  and cloud instances as named environments (plugin v0.19+). The project
  id is recorded at `[stack.projects.stripe].project` after first
  creation — the reproducibility anchor that lets any fresh checkout
  re-link with a `pull`.
- **Per-instance resources derive from the definition:** datastores →
  `render/postgres`, services → `render/web-service` or
  `render/static-site` per their `[services.X.render]` config, and
  integrations with `provider = "clerk"` → `clerk/auth` (optionally
  enabling Clerk Organizations and slugs for app/test fixtures). Cloud
  resource names are
  `{stack}-{instance}-{service}`, DNS-safe by construction (§2 name
  rules).
- **Stripe Projects provisions and tracks spend; the Render REST API
  fills its gaps:** interpolated env vars, SPA rewrite routes, deploy
  triggers, deploy polling with per-kind timeouts (a Rust release build
  can take 30+ minutes on small tiers), and the health wait. JSON-mode
  CLI calls with the plain-mode fallbacks `cloud-env.ts` had to learn
  are part of the backend, not rediscovered per stack. The Render API
  key resolves from env or a scoped key file.
- **Sequencing per instance:** provision hosted integrations →
  provision datastores → run `prepare` hooks from the operator's
  machine against external connection strings → push env vars → deploy
  services → health gate → `up`.
- **Every step checkpoints into the manifest before proceeding** (§2);
  interrupted runs resume rather than duplicate.
- **Teardown is verified, dependents-first:** remove services, then
  datastores, delete the named environment — then query the Render API
  for survivors and **exit non-zero listing them if anything that bills
  or holds state remains** (the vision demands refusal here, not the
  warning `cloud-env.ts` settled for). Spend is printed after every
  `up` and `down`.
- **Stripe Projects is the authoritative inventory for recovery.** If
  the state store and reality drift — a lost local state file, a
  hand-deleted service — `stripe projects pull` re-links a fresh
  checkout or machine to the stack's project, and `services list
  --json` is the inventory the §2 reconcile-against-observation
  machinery compares against, so orphans are found and either
  re-adopted or torn down rather than leaked. The fleet plane (§2)
  makes this rare; this path exists so it is never fatal.
- **Paid tiers are never auto-confirmed** — `--confirm-paid` per
  invocation (§2), backed by **hard spend caps**: the backend sets a
  per-provider spend limit on the stack's project
  (`stripe projects billing update --limit <amount> --provider render`)
  so a leak is bounded even if reaping fails. Tested present in plugin
  v0.19.0 on 2026-06-11 — available from day one.
- **Plugin surface, tested against v0.19.0 on 2026-06-11** (the latest
  distributable that day, hours after Stripe's expansion
  announcement — their docs already describe a newer, not-yet-shipped
  surface): present and working — `--json` (clean
  `{ok, command, version, data}` envelope), `--yes`, `--stream`,
  `billing update --limit [--provider]`, `rotate`, `catalog`/`search`,
  `init --from <URL>`. **Not yet shipped** — the documented
  `--no-interactive`/`--auto-confirm`/`--quiet` flags and the `share`/
  `import` commands (`init --from` exists but nothing can generate a
  share URL yet — the same gap atto's `config.ts` recorded). Until the
  new flags land, the backend keeps `cloud-env.ts`'s plain-mode
  fallbacks for `--json`'s confirmation/auth quirks; re-test on each
  plugin release. Environment membership (`env add`/`env remove`) can
  express a resource shared across instances — the seam for
  invariant 8's "unless the definition explicitly says so" (not in
  the v0 schema).

## 4b. Vercel substrate

Mirrors §4's Stripe + provider-API split for
[Vercel](https://vercel.com) git-backed projects.

- **Same Stripe project per stack** as Render and hosted integrations
  (§4). Per-instance resources are named environments; cloud resource
  names remain `{stack}-{instance}-{service}`.
- **Catalog resources:** services → `vercel/project` with
  `{"name": "<resource-name>"}` only at provision time; optional
  stack-level `vercel/pro` when `[stack.vercel].plan = "pro"` (paid →
  `--confirm-paid`). Hobby projects are free-tier catalog resources.
- **Stripe Projects provisions; the Vercel REST API operates:** after
  Stripe creates/links the project, stackless pushes interpolated env
  vars, creates a git deployment from the pinned `ref` and
  `[services.X.vercel]` build settings, polls until `READY`, runs the
  health gate against the deployment URL, and on `down` deletes the
  project and polls until gone. The Vercel API token resolves from
  `VERCEL_TOKEN` or a scoped `.vercel-token` file next to the
  definition (`VERCEL_TEAM_ID` optional for team-scoped projects).
- **No datastores on Vercel in v0** — postgres and other managed state
  belong on `local` or `render` today.
- **No root-origin alias on cloud** (same as Render): every service
  keeps its own deployment URL; `${services.X.origin}` resolves
  accordingly.
- **Setup is skipped** (cloud builds run in Vercel's pipeline);
  **prepare** runs on the operator's machine from a shallow git clone,
  same as Render (§1).
- **Spend caps and summaries** follow §4 (`billing update --provider
  vercel`; spend line printed after cloud `up`/`down`).
- **`stackless logs` is not wired for Vercel in v0** — use the Vercel
  dashboard; Render and local substrates support the `logs` verb.

## 5. Trust boundary (phased, post-v0) — TBD

Recorded from design discussion, to be developed when sequenced:

- Two separable jobs: (1) default-deny egress — an SNI-passthrough
  gateway on a per-instance network, no MITM, no app changes; (2) secret
  blinding — token→real-value swap at egress.
- Two-class secret model: **instance-minted** secrets (DB passwords,
  per-instance keys) are protected by leasing alone — they die with the
  instance and are useless outside it; blinding them buys nothing.
  **Third-party durable** secrets are the dangerous class and the only
  candidates for blinding.

## 6. Leases & the reaper

The reaper lives in the local daemon (see §3), which
makes lease expiry a system guarantee across reboots via launchd/
systemd keep-alive. It enforces leases for instances on **all**
substrates — a cloud instance's lease is reaped from the operator's
machine through the same teardown path `down` uses. Known gap, recorded
honestly: if the operator's machine is off/asleep past a cloud lease's
expiry, the instance outlives its lease until the machine returns (the
daemon reaps overdue leases immediately on start/wake). A
substrate-side backstop is a candidate for later.

Lease semantics:

- **Every instance carries a lease from birth** — `--lease <duration>`
  at `up`, with per-substrate defaults (local: 24h; Render and Vercel:
  8h — cloud instances bill, so abandonment must be expensive to nobody).
- **The lease renews to its full duration at the start of every
  mutating verb, and again on a successful `up` or `verify`.**
  "In use" means exactly "recently verbed": `verify` is the keepalive
  an agent runs mid-work — it renews *and* proves health, which is
  what the agent wants anyway. Traffic through an instance
  deliberately does not renew: a forgotten browser tab with a polling
  SPA must not keep an instance alive forever. Renewal is not new
  spend — consent at creation covers renewals within the lease model,
  and the project's hard spend cap (§4) bounds total exposure
  regardless. No separate renew verb in v0.
- **The reaper ticks every minute** and sends overdue instances through
  the same verified-teardown path as `down`. It never reaps an
  instance holding its operation lock (§2) — an operation that
  outlives its whole lease finishes before the reaper acts. A failed
  reap retries with backoff and is surfaced in `list`/`status` until
  it succeeds — silence is not success (invariant 4).
- **`list` shows remaining lease** for every instance, both substrates.

## 7. Health & proof

The contract shapes were decided in §1; this is how they execute.

- **Per-service health checks run through the instance's public
  origin** — locally the proxy; on Render the `onrender.com` URL; on
  Vercel the recorded deployment URL — never the raw port, so routing
  is part of what "healthy" proves. Shape:
  `health = { path, status = 200, contains = "..." }` with a retry
  budget defaulting per substrate (seconds locally; minutes against a
  cold cloud deploy, as `cloud-env.ts`'s 5-minute health wait proved
  necessary). This covers both atto checks: `/health` == `ok` and the
  SPA shell containing `id="root"`.
- **Datastore readiness is built in per engine** (postgres:
  `pg_isready`); definitions never declare it.
- **`up` reports staged truth:** provisioned → configured → prepared →
  started → healthy, each stage gated on the previous (invariant 2).
  `status` shows the stage an instance actually reached, per service.
- **`verify` runs the stack's verify command** with env built by the
  same interpolation mechanism services use (`[stack.verify]` has
  `run` and `env` with `${...}` references) — origins, instance name,
  whatever the smoke tier needs. App-level fixtures (atto's Clerk e2e
  user/org) are the stack's own business, created in `prepare` or in
  the verify command itself; stackless stays unopinionated about them.

## 8. Crate layout

One Cargo workspace; the seams mirror the load-bearing boundaries above
so each substrate compiles and tests in isolation.

Workspace-wide conventions:

- **Parsing/CLI foundations are pinned:** the `toml` crate (with serde)
  parses `stackless.toml`; `clap` (derive) owns the CLI surface.
- **Errors are `thiserror` enums end-to-end — no untyped propagation**
  (no `anyhow` in any crate, including the binary). The reason is the
  §2 error contract: every error must carry its stable code, the
  failed step/instance context, and a remediation, all the way from
  where it occurs to where it's serialized — an opaque boxed error
  can't. Each crate defines its own error types; the CLI layer maps
  them onto the agent-facing `{schema_version, code, message, step,
  instance, context, remediation}` shape and exit codes. A new error
  variant is only complete when its remediation text says what the
  operator should actually do.

- **`stackless-core`** — the definition model (serde + validation,
  interpolation, the derived dependency graph), the SQL state store
  (instance records, leases, operation locks, checkpoint journal;
  local file via `rusqlite` — bundled SQLite, WAL; `libsql` remote
  mode for the opt-in fleet plane — §2), and the lifecycle
  engine: plan steps, checkpoint, reconcile recorded state against
  observation. The engine is shared by `up`, resume, daemon adoption,
  and the reaper — they are the same machinery. Defines the
  `Substrate` trait.
- **`stackless-local`** — `Substrate` impl: process spawn/adoption,
  the container runner (the `bollard` crate against the Docker Engine
  socket — typed API beats parsing CLI output; we resolve the socket
  path/`DOCKER_HOST` ourselves since contexts are a CLI-side concept),
  port allocation. Source materialization uses **`gix` (gitoxide)
  in-process** (verified 2026-06): one bare cache repo per source
  (gix clone/fetch — credential-helper support is real; SSH spawns the
  system `ssh`, inheriting user config), then per instance a thin
  `gix init` repo whose `objects/info/alternates` points at the cache
  (object sharing without copies), HEAD set to the pinned commit, and
  a `gix-worktree-state` checkout (multi-threaded, git-speed, the
  engine cargo's gitoxide path ships on). Linked-worktree *creation*
  (`git worktree add`) is the one thing gix lacks — and this flow
  doesn't need it: instances run pinned refs, they never commit back,
  and the alternates repo is real enough for builds that shell out to
  `git rev-parse`. No git CLI dependency, no fork; if true linked
  worktrees are ever wanted, contribute the admin plumbing upstream
  rather than fork a 100-crate fast-moving workspace.
- **`stackless-stripe-projects`** — neutral Stripe Projects CLI
  driver: project anchor (`[stack.projects.stripe]`), per-instance
  environments, catalog add/remove, env materialization, spend caps.
- **`stackless-integrations`** — hosted integration routing and
  provider adapters (Clerk today); substrates call here for provision /
  observe / destroy. Each provider declares managed vs host-bound
  hosting and config scope via the `Hostable` trait.
- **`stackless-render`** — `Substrate` impl and Render REST client.
- **`stackless-vercel`** — `Substrate` impl and Vercel REST client.
- **`stackless-daemon`** — unix-socket RPC server, process
  bookkeeping, the reaper tick, and the reverse proxy (hyper,
  Host-header routing).
- **`stackless`** (bin) — the clap CLI: substrate registry, daemon
  client/spawner, human and `--json` output.
