# `stackless.toml` — the complete schema reference

This document is sufficient on its own to write a valid stack
definition. It describes what the implementation actually enforces;
every rule here is checked by `stackless check <file>` (add
`--on <substrate>` for substrate-specific completeness; registered
hosts are listed in [PROVIDERS.md](../PROVIDERS.md))
before anything provisions. Validation failures carry stable
machine-readable codes (listed throughout) plus a remediation.

A definition lives at the repo root as `stackless.toml`. One file is
enough: a stranger (or an agent) holding only a repo with this file
can run `stackless up --name <name> --on local` (or omit `--name` for
an auto-assigned `{stack.name}-{uuid}`) and get a working, isolated
copy of the product.

## Minimal valid definition

```toml
[stack]
name = "hello"

[services.web]
source = { repo = "https://github.com/you/hello", ref = "main" }
root_origin = true
health = { path = "/", contains = "hello" }

  [services.web.local]
  run = "python3 -m http.server $PORT --bind 127.0.0.1"
```

### Minimal Vercel definition

```toml
[stack]
name = "hello"

[services.web]
source = { repo = "https://github.com/you/hello", ref = "main" }
health = { path = "/", contains = "hello" }

  [services.web.vercel]
  framework = "vite"
  build = "npm run build"
```

Requires `VERCEL_TOKEN` (or `.vercel-token`) and a GitHub repo your
Vercel team can deploy. No `[services.web.local]` block is required
if you only deploy to Vercel, but `stackless check --on local` will
report missing local config.

## Top level

Exactly five sections are recognized; any other top-level key is
rejected (`def.parse.schema`):

| Section | Required | Purpose |
|---|---|---|
| `[stack]` | yes | Stack identity, verify contract, per-substrate stack config |
| `[secrets]` | no | The required-secrets list |
| `[integrations.<name>]` | no | Hosted third-party services provisioned through Stripe Projects |
| `[services.<name>]` | no* | The application services |

*At least one service must be declared (`def.validate.no_services`).

### Naming rules (everywhere)

Stack, service, and instance names become hostnames and
cloud resource names, so they must be DNS-safe: lowercase letters,
digits, and hyphens; starting with a letter; not ending with a hyphen;
at most 63 characters (`def.validate.name_invalid`).

```
^[a-z][a-z0-9-]*$   (no trailing '-', length ≤ 63)
```

## `[stack]`

```toml
[stack]
name = "atto"            # required, DNS-safe

[stack.verify]           # optional: the proof contract (see below)
run = "bun e2e/smoke.ts"
env = { ATTO_STACKLESS = "1", ATTO_E2E_WEB_ORIGIN = "${services.web.origin}" }

[stack.projects.stripe]  # optional: shared Stripe Projects anchor
project = "project_..."  # written back by stackless after first project creation

[stack.render]           # optional: per-substrate stack config
region = "oregon"

[stack.vercel]           # optional: per-substrate stack config
plan = "hobby"           # hobby (default) or pro (paid → requires --confirm-paid)

[stack.fly]              # optional: per-substrate stack config
region = "iad"           # Fly region (default iad)
```

- `name` — required.
- `verify` — optional. `run` (string) is the default tier executed by
  `stackless verify <instance>` (or `stackless verify <instance> --tier default`).
  `env` (table of strings, interpolation allowed) is resolved and exported.
  The command runs from the materialized source of the `root_origin` service
  (or the first service if none). Output is captured to a per-instance log file;
  `--json` success includes `duration_ms`, `exit_status`, and `log_path`.
  Named tiers live under `[stack.verify.tiers.<name>]` with the same `{run, env}`
  shape; `<name>` must be DNS-safe (same rules as service names). Unknown tiers
  fail with `verify.tier_unknown`. Declaring no `[stack.verify]` and no tiers
  makes `stackless verify` fail with `verify.not_declared`.
- `projects.stripe.project` — optional. Stackless writes this after it
  creates or adopts the stack's Stripe Project. Local integrations and
  cloud resources (Render, Vercel, Clerk, …) share this anchor;
  re-link a fresh checkout with `stripe projects pull <id>`.
- Any other key under `[stack]` must be the name of a registered
  substrate (`local`, `render`, `vercel`, `fly`, `netlify`, `railway`,
  `cloudflare`, `wordpress`, `laravel-cloud`, `gitlab`) and must be a table
  (`def.validate.unknown_key`, `def.validate.substrate_block_invalid`).
- `[stack.render]` — optional. `region` defaults to `"oregon"` (Render
  substrate only).
- `[stack.vercel]` — optional. `plan` defaults to `"hobby"`. `"pro"`
  provisions the Vercel Pro plan through Stripe Projects and requires
  `--confirm-paid` (`vercel.payment.not_confirmed`).
- `[stack.fly]` — optional. `region` defaults to `"iad"` (Fly substrate
  only).

## `[secrets]`

```toml
[secrets]
required = ["GITHUB_PACKAGES_TOKEN"]
```

- `required` — list of secret names the stack needs. Every key
  referenced anywhere (`${secrets.KEY}` or a service's `secrets`
  list) **must** appear here (`def.validate.secret_not_required`).
- Resolution at `up`/`verify` time: when `[stack.projects.stripe].project`
  is recorded, stackless refreshes the Stripe Projects vault (`.env` /
  `.env.<instance>` written by `stripe projects env --pull`) as the
  **base**. Shared team secrets belong in backend-backed project variables
  (`stripe projects variables set <name> --env-key <KEY>`). A gitignored
  **`.stackless.env`** next to `stackless.toml` (`KEY=value` lines)
  **overlays** the vault — the file wins. Local-only stacks without a
  Stripe anchor use `.stackless.env` only. A required key resolving from
  no source fails before anything provisions (`secrets.unresolved`). Run
  `stackless doctor` for aggregated Stripe Projects `--preflight` blockers.
- The directory the definition came from is recorded at instance
  creation, so resume and `verify` find the same `.stackless.env`
  regardless of the invoking shell's working directory.

## `[integrations.<name>]`

```toml
[integrations.clerk]
provider = "clerk"
app_name = "${stack.name}-${instance.name}"
credential_set = "development"
organizations = true
```

- The table key (`clerk` in `[integrations.clerk]`) is the **logical
  slot** — the name used in `${integrations.clerk.*}` references.
  `provider` is the **catalog adapter** (`clerk` → `clerk/auth` via
  Stripe Projects, internal to stackless). A later slot like
  `[integrations.auth]` with `provider = "clerk"` does not require a
  schema change.
- Each provider declares a **hosting model** in the integrations registry:
  - **Managed**: runs on the provider's cloud, not on your stack host.
    Config is **global only** — per-host tables like
    `[integrations.<name>.render]` are rejected (`integration.config.invalid`).
    Managed integrations are available on every `--on` host.
  - **Host-bound**: tied to explicit stack hosts (`local`, `render`,
    `vercel`, …). `stackless check --on <host>` rejects providers not
    listed for that host (`integration.host.unsupported`). Host-bound
    providers may allow per-host override tables when declared.
- `${integrations.<name>.<output>}` references are **ordering edges**
  in the derived dependency graph: integrations provision before any
  service whose env references them.
- Every provider registered in
  `crates/stackless-integrations/src/registry.rs` is first-class.
  Stackless provisions through Stripe Projects and exposes outputs
  through interpolation. Provider-specific fields are validated by
  `stackless-integrations` during `stackless check`.
- `provider` — required string naming a known catalog provider (any
  registry entry).
- Provider-specific keys vary by adapter. For `provider = "clerk"` (example
  below): `app_name` (required, interpolation allowed);
  `credential_set` (optional, `"development"` by default; `"production"`
  requires `production_domain`); `production_domain` (optional);
  `organizations` (optional boolean, `false` by default — when `true`,
  enables Clerk Organizations / slugs so tests can create tenant org
  fixtures named by `${instance.name}`). Stackless does not create
  app-specific Clerk orgs/users; the application/test layer owns that.

### Delivering integration credentials to tests

Stackless injects `${integrations.<name>.<output>}` into service and verify
env. Out-of-process tests that need those values have two supported paths:

1. **Verify-tier env (preferred when secrets must stay off stdout).** Put the
   prove command under `[stack.verify]` (or a tier) and interpolate into its
   `env` map. The child process receives credentials; the verify JSON envelope
   does not echo them.
2. **`up --json` / SDK `UpOutcome.integrations`.** After a successful `up`,
   checkpoint outputs are returned as a nested object
   (`{ "clerk": { "secret_key": "…", "publishable_key": "…" } }`, omitted when
   empty). Generated `bindIntegrations` types that map. Capturing `up --json`
   stdout captures credential values; redact CI logs or use path 1 instead.

Do not scrape vault files or `state.db` for Clerk keys. That is unsupported
glue, not a stackless API.

## `[services.<name>]`

```toml
[services.api]
source = { repo = "https://github.com/you/api", ref = "main" }   # required
setup = "mise install"            # optional: once, after materialization
prepare = "just seed"             # optional: every up, after deps are ready
secrets = ["API_TOKEN"]            # optional: injected as same-named env vars
env = { CLERK_SECRET_KEY = "${integrations.clerk.secret_key}", RUST_LOG = "info" }
health = { path = "/health", contains = "ok" }                   # required
root_origin = false               # optional, at most one service true
```

| Key | Required | Type | Meaning |
|---|---|---|---|
| `source` | yes | `{ repo, ref }` | Git source: `repo` is a URL (https, file://, ssh), `ref` a branch, tag, or commit SHA. Materialized per instance from a shared cache. |
| `setup` | no | string | Shell command run **once** after the source is materialized (toolchain, deps — e.g. `mise install && bun install`). Re-run if interrupted; must be idempotent. |
| `prepare` | no | string | Shell command run on **every** `up`, after the service's dependencies are ready and before the service starts (migrations, seed). On cloud substrates it runs on the operator's machine against external connection strings. Must be re-run-safe. If your service migrates on boot, omit migration here. |
| `secrets` | no | string list | Each name must be in `[secrets].required`; injected into the service's environment as a same-named variable. |
| `env` | no | string table | The service's environment. Values may interpolate (next section). This is also where dependencies are expressed. |
| `health` | yes | `{ path, status?, contains? }` | The health contract gating `up` (below). |
| `root_origin` | no | bool | Exactly zero or one service may set this; it additionally claims the instance's root origin (`http://{instance}.localhost:4444` locally). Give it to the user-facing web service. (`def.validate.root_origin_conflict`) |

Any other key under a service must be a registered substrate name with
a table value. `depends_on` is rejected outright
(`def.validate.depends_on_rejected`): a dependency must be expressed
in wiring — reference the dependency from `env` and the startup order
is derived from that.

### `health` — the contract gating `up`

```toml
health = { path = "/health" }                          # expect HTTP 200
health = { path = "/health", contains = "ok" }         # 200 AND body contains "ok"
health = { path = "/", status = 200, contains = 'id="root"' }  # SPA shell check
```

- `path` — required, the HTTP path probed.
- `status` — optional, default `200`.
- `contains` — optional substring the body must contain.

Checks run **through the instance's public origin** (the proxy
locally, the `onrender.com` URL on Render) — never the raw port — so
routing is part of what "healthy" proves. Retry budgets: 300 s
locally (generous because `cargo run`-style commands compile before
serving; a dead process fails fast), 5 min against a cold cloud
deploy. `up` refuses to report success until every service passes
(`local.health_failed`, `local.service_died`).

### `[services.<name>.local]` — how the local substrate runs it

```toml
  [services.api.local]
  run = "cargo run"                       # required
  env = { TENANT_SLUG = "local-dev" }     # optional overlay, wins over common env
```

- `run` — required non-empty shell command. Executed in the service's
  materialized source directory with the resolved env plus **`$PORT`**
  (an OS-allocated port the service must bind on `127.0.0.1`). The
  command's whole process group is the unit of supervision and
  teardown, so wrappers like `cargo run` are fine.
- `env` — optional string table overlaying the common `env`
  (overlay wins). Interpolation allowed.
- No other keys are accepted (`local.config.invalid`).

### `[services.<name>.render]` — how Render runs it

Two shapes. A web service:

```toml
  [services.api.render]
  runtime = "rust"
  build = "cargo build --release"
  start = "./target/release/api-server"
  env = { SQLX_OFFLINE = "true" }    # optional overlay
```

Or a static site:

```toml
  [services.web.render]
  static = { build = "bun install && bun run build", publish = "./dist", spa_rewrite = true }
  env = { BUN_VERSION = "1.3.11", GITHUB_PACKAGES_TOKEN = "${secrets.GITHUB_PACKAGES_TOKEN}" }
```

- `spa_rewrite = true` installs the `/* → /index.html` rewrite.
- `runtime` passes Render's runtime vocabulary through (`rust`,
  `node`, ...).
- Cloud resource names are `{stack}-{instance}-{service}`; origins are
  `https://{stack}-{instance}-{service}.onrender.com`.
- `up --on render` requires every service to carry a `render` block
  (`def.validate.substrate_config_missing`), refuses `--source` pins
  (`engine.source_override.unsupported` — commit and push, then pin
  the ref), and requires `--confirm-paid` for anything that bills
  (`render.payment.not_confirmed`).

### `[services.<name>.vercel]` — how Vercel runs it

```toml
  [services.web.vercel]
  framework = "vite"                    # optional Vercel framework preset
  build = "npm run build"               # optional build command override
  install = "npm install"               # optional install command override
  root = "apps/web"                     # optional monorepo root directory
  output = "dist"                       # optional output directory
  deploy = "git"                        # "git" (default) or "upload"
  env = { VITE_API_ORIGIN = "${services.api.origin}" }  # optional overlay
```

- `deploy` controls how the source reaches Vercel. `"git"` (the default)
  has Vercel pull the repo via `gitSource` — this requires the
  Stripe-managed Vercel team be connected to GitHub. `"upload"` has
  stackless clone the pinned ref itself and post the files inline, so no
  Vercel↔GitHub connection is needed.
- With `deploy = "git"`, `source.repo` must be a public GitHub HTTPS
  remote (`https://github.com/org/repo`). `deploy = "upload"` only needs
  a repo stackless can clone.
- Cloud resource names are `{stack}-{instance}-{service}`; the live
  origin is the deployment URL recorded at `start` (typically
  `https://{name}.vercel.app`).
- `up --on vercel` requires every service to carry a `vercel` block
  (`def.validate.substrate_config_missing`), refuses `--source` pins
  (`engine.source_override.unsupported`), and requires `--confirm-paid`
  when `[stack.vercel].plan = "pro"` (`vercel.payment.not_confirmed`).
- Setup is skipped on cloud (same as Render); prepare runs on the
  operator's machine from a shallow `git clone` of the pinned ref.
- **Two layers (same as Render):** Stripe Projects provisions the
  `vercel/project` catalog resource; the Vercel REST API handles env,
  deploy, health, and teardown. Stripe does not replace the API token —
  set `VERCEL_TOKEN` or write `.vercel-token` next to the definition
  (`vercel.api_key.missing`). Optional `VERCEL_TEAM_ID` for team-scoped
  projects.
- `stackless logs` is not wired for Vercel in v0 — use the Vercel
  dashboard.

### `[services.<name>.fly]` — how Fly runs it

Fly v0 is **image-only**: a service declares a prebuilt container image
and the substrate runs it as a Fly machine.

```toml
  [services.web.fly]
  image = "ghcr.io/org/web:latest"      # required: prebuilt container image
  internal_port = 8080                  # optional, the port the container listens on (default 8080)
  cmd = ["server", "--port", "8080"]    # optional: override the image CMD (container args)
  cpu_kind = "shared"                   # optional machine preset (default shared)
  cpus = 1                              # optional (default 1)
  memory_mb = 256                       # optional (default 256)
  env = { API_ORIGIN = "${services.api.origin}" }  # optional overlay
```

- Cloud resource names are `{stack}-{instance}-{service}` — also the Fly
  app name, so it must be a legal app name (`^[a-z][a-z0-9-]{2,62}$`);
  origins are `https://{stack}-{instance}-{service}.fly.dev`.
- `up --on fly` requires every service to carry a `fly` block
  (`fly.config.invalid`), refuses `--source` pins
  (`engine.source_override.unsupported`), and requires `--confirm-paid`
  (`fly.payment.not_confirmed` — `flyio/app` is usage-billed).
- Setup is skipped on cloud (same as Render); prepare runs on the
  operator's machine from a shallow `git clone` of the pinned ref.
- **Two layers (same as Render):** Stripe Projects provisions the
  `flyio/app` catalog resource and returns a Stripe-managed, app-scoped
  deploy token; the Fly Machines REST API uses that token to allocate the
  app's public IPs, create the machine, and poll it to `started`. No
  operator API token is needed — the credential comes from provisioning,
  and `observe`/`down` use the Stripe resource registration.
- `stackless logs` is not wired for Fly in v0 — use the Fly dashboard.

### `[services.<name>.netlify]` — how Netlify runs it

Netlify v0 is **static upload**: the substrate clones the pinned ref and
uploads the files under `root` (or the repo root) as a Netlify deploy. The
block is optional.

```toml
  [services.web.netlify]
  root = "dist"                         # optional: subdir to publish (default: repo root)
```

- Cloud resource names are `{stack}-{instance}-{service}` — also the Netlify
  site name; origins are `https://{stack}-{instance}-{service}.netlify.app`
  (the real URL is recorded from the deploy's `ssl_url`).
- `up --on netlify` refuses `--source` pins
  (`engine.source_override.unsupported`). `netlify/project` is **free**, so no
  `--confirm-paid` is required.
- Setup is skipped on cloud; prepare runs on the operator's machine from a
  shallow `git clone` of the pinned ref.
- **Two layers (same as Render):** Stripe Projects provisions the
  `netlify/project` catalog resource and returns a Stripe-managed token + site
  id; the Netlify REST API runs the file-digest deploy (SHA1 per file, PUT only
  the files Netlify still needs) and polls to `ready`. No operator API token is
  needed; `observe`/`down` use the Stripe resource registration.
- `stackless logs` is not wired for Netlify in v0 — use the Netlify dashboard.

## The interpolation namespace

Env values (common `env`, substrate `env` overlays, and
`[stack.verify].env`) may reference exactly these forms:

| Reference | Resolves to |
|---|---|
| `${stack.name}` | the stack's declared name |
| `${instance.name}` | the instance's name — the one identity everything derives from |
| `${services.X.origin}` | service X's substrate-appropriate origin. Local: `http://x.{instance}.localhost:4444` (the root-origin service resolves to `http://{instance}.localhost:4444` — what browsers actually use). Render: `https://{stack}-{instance}-x.onrender.com`. Vercel: the deployment URL after `start` (best-effort `https://{stack}-{instance}-x.vercel.app` before deploy). Fly: `https://{stack}-{instance}-x.fly.dev`. Netlify: the deploy's `ssl_url` recorded at `start` (`https://{stack}-{instance}-x.netlify.app`) |
| `${secrets.KEY}` | the resolved secret value (KEY must be in `[secrets].required`) — for renaming; the `secrets = [...]` list already injects same-named vars |
| `${integrations.clerk.secret_key}` | the Clerk secret key selected from Stripe Projects' Clerk environments JSON (`CLERK_AUTH_ENVIRONMENTS` or `CLERK_ENVIRONMENTS`) |
| `${integrations.clerk.publishable_key}` | the Clerk publishable key selected from Stripe Projects' Clerk environments JSON (`CLERK_AUTH_ENVIRONMENTS` or `CLERK_ENVIRONMENTS`) |

`$PORT` is **not** an interpolation reference — it is injected by the
local substrate into `run` commands only, and passes through env
values untouched.

Rules, all enforced at parse/validate time (never at `up` time):

- Referencing an undeclared service/secret →
  `def.validate.undeclared_reference` / `def.validate.secret_not_required`.
- Any other `${...}` form → `def.validate.reference_syntax`.
- An unterminated `${...` → `def.validate.reference_syntax`.

### Wiring derives the graph — there is no `depends_on`

If service A's env references `${services.B.origin}`, that is recorded
wiring but never an ordering constraint — origins are derivable from
the instance name alone, so mutual references (api ↔ web CORS) are
legal and are not cycles. `stackless check` prints the derived startup
order and wiring edges.

## What happens on `up` (so you can write definitions that fit it)

Per instance: provision hosted integrations → materialize each service's
source (shared git cache; or your `--source` pin; with `--dirty`, snapshot
the pin's working tree into instance-owned space) → `setup` (once) →
`prepare` (every up, deps ready) → start → health-gate. Every step
checkpoints before proceeding, so interrupted runs **resume** — re-running
`up` re-checks recorded resources against reality and re-executes only what
is missing; `prepare` and health gates rerun on every `up` by contract.

`up` on an existing name resumes it; the substrate was fixed at
creation and is never asked again. Names are unique across substrates:
creating `demo` on render while a local `demo` exists is an error
(`state.instance.exists`), not a sibling.

## Common stable error codes

Agents: branch on `error.code` from `--json` output, never on prose.
The full registry lives in `crates/stackless-core/src/fault.rs`;
codes are versioned API surface. The ones a definition author hits
most:

| Code | Meaning |
|---|---|
| `def.parse.syntax` | not valid TOML |
| `def.parse.schema` | valid TOML, unknown/missing/mistyped field (includes a missing required `health` or `source`) |
| `def.validate.name_invalid` | a name is not DNS-safe |
| `def.validate.unknown_key` | a key that is neither a schema field nor a registered substrate (typos land here) |
| `def.validate.depends_on_rejected` | use wiring, not `depends_on` |
| `def.validate.undeclared_reference` | `${...}` names something not declared |
| `def.validate.secret_not_required` | a secret used but not in `[secrets].required` |
| `def.validate.integration_invalid` | an `[integrations.*]` block is structurally invalid (missing `provider`, bad host override) |
| `integration.config.invalid` | provider config/outputs or per-host override policy fail registry validation (`stackless check`) |
| `integration.host.unsupported` | a host-bound integration's provider is not supported on the selected `--on` host |
| `def.validate.substrate_config_missing` | `up --on X` but a service has no `[services.*.X]` block |
| `def.validate.root_origin_conflict` | more than one `root_origin = true` |
| `secrets.unresolved` | a required secret resolved from no source |
| `state.lock.held` | another operation is running on this instance — wait and retry |
| `engine.source_override.unsupported` | `--source` or `--dirty` on a cloud substrate |
| `verify.source_unavailable` | `stackless verify` could not find or materialize the command cwd |
| `render.payment.not_confirmed` | re-run with `--confirm-paid` |
| `vercel.payment.not_confirmed` | re-run with `--confirm-paid` (required for `plan = "pro"`) |
| `vercel.api_key.missing` | set `VERCEL_TOKEN` or write `.vercel-token` next to the definition |
| `vercel.deploy.failed` | inspect Vercel deployment logs and re-run `stackless up` |
| `vercel.deploy.timeout` | wait for the build or fix it, then re-run `stackless up` |
| `vercel.health.failed` | fix the health contract and re-run `stackless up` |
| `vercel.teardown.survivor` | remove the project from the Vercel dashboard or re-run `stackless down` |
| `integration.host.unsupported` | pick a supported `--on` host or change the integration provider |

## Full annotated example

The canonical dogfood — SPA + Rust API + Postgres + third-party auth:

```toml
[stack]
name = "atto"

[stack.render]
region = "oregon"

[stack.projects.stripe]
project = "project_61UqVKJia4fs..."   # recorded by stackless after first project creation

[stack.verify]
run = "bun e2e/smoke.ts"              # default tier
env = { ATTO_STACKLESS = "1", ATTO_E2E_WEB_ORIGIN = "${services.web.origin}", ATTO_E2E_API_ORIGIN = "${services.api.origin}", ATTO_E2E_TENANT_SLUG = "${instance.name}", CLERK_SECRET_KEY = "${integrations.clerk.secret_key}", VITE_CLERK_PUBLISHABLE_KEY = "${integrations.clerk.publishable_key}" }

[stack.verify.tiers.integration]
run = "bun e2e/integration.ts"
env = { ATTO_E2E_WEB_ORIGIN = "${services.web.origin}" }

[secrets]
required = ["GITHUB_PACKAGES_TOKEN"]

[integrations.clerk]
provider = "clerk"
app_name = "${stack.name}-${instance.name}"
credential_set = "development"
organizations = true

[services.api]
source = { repo = "https://github.com/haaku-co/atto-server", ref = "main" }
setup = "mise install"
prepare = "just migrate-run && just seed"
env = { CORS_ALLOWED_ORIGINS = "${services.web.origin}", TENANT_SLUG = "${instance.name}", CLERK_SECRET_KEY = "${integrations.clerk.secret_key}", RUST_LOG = "info" }
health = { path = "/health", contains = "ok" }

  [services.api.local]
  run = "cargo run"                   # $PORT injected; runs in the materialized source

  [services.api.render]
  runtime = "rust"
  build = "cargo build --release"
  start = "./target/release/atto-server"
  env = { SQLX_OFFLINE = "true", RUSTUP_TOOLCHAIN = "1.94.0" }

[services.web]
source = { repo = "https://github.com/haaku-co/atto-web", ref = "main" }
setup = "mise install && bun install"
root_origin = true                    # claims http://{instance}.localhost too
env = { VITE_API_ORIGIN = "${services.api.origin}", VITE_CLERK_PUBLISHABLE_KEY = "${integrations.clerk.publishable_key}" }
health = { path = "/", contains = 'id="root"' }

  [services.web.local]
  run = "bunx vite --host 127.0.0.1 --port $PORT --strictPort"

  [services.web.render]
  static = { build = "bun install && bun run build", publish = "./dist", spa_rewrite = true }
  env = { BUN_VERSION = "1.3.11", GITHUB_PACKAGES_TOKEN = "${secrets.GITHUB_PACKAGES_TOKEN}" }
```

## Providers

Stripe Projects is internal plumbing — never declared in
`stackless.toml`. Registered hosting substrates (`--on`) and catalog
integrations (`provider`) are listed in [PROVIDERS.md](../PROVIDERS.md);
that file is maintained from the registries.

### Platform

- [x] `stackless logs` (local)
- [x] `stackless logs` (render)
- [ ] `stackless logs` (vercel)
- [ ] fleet state plane (Turso Cloud)

## Checklist for agents writing a definition

1. Every service: `source` + `health` + a `[services.X.local]` block
   with `run`. Add `[services.X.render]` and/or `[services.X.vercel]`
   for each cloud host you intend to use.
2. Express every dependency as an env reference; never invent ordering
   keys.
3. The service must bind `$PORT` on `127.0.0.1` when run locally.
4. Put every hand-managed secret you reference into `[secrets].required`;
   use `[integrations.clerk]` with `provider = "clerk"` for Clerk app
   credentials, and set `organizations = true` when tests need Clerk
   org fixtures. Do not add `[integrations.clerk.render]` or other
   per-host integration tables — Clerk is managed/global-only.
5. Exactly one user-facing service gets `root_origin = true` (local
   substrate only; cloud keeps per-service origins).
6. Declare a `[stack.verify]` command that proves the stack works
   end-to-end — health checks prove liveness; verify proves behavior.
7. Run `stackless check stackless.toml --on local` (and `--on render`
   / `--on vercel` for each cloud target) and fix what it reports; it
   validates everything this document describes without provisioning
   anything.
8. Cloud deploys: commit and pin refs (`--source` is local-only). Vercel
   requires `source.repo` as `https://github.com/org/repo`. Set
   `RENDER_API_KEY` / `VERCEL_TOKEN` (or scoped key files) before
   `stackless up --on render` / `--on vercel`.
