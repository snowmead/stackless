---
name: stackless
description: >-
  Install stackless, author stackless.toml, and run the full ephemeral-stack
  lifecycle (check, up, verify, status, logs, down) with --json. Branch on
  error.code, never prose. Use for stackless.toml, local/cloud up, and agent
  automation against the stackless CLI.
---

# stackless agent skill

Ephemeral software stacks: named, leased, isolated, proven, destroyed.
The schema reference is [docs/SCHEMA.md](../../docs/SCHEMA.md);
this skill covers install, authoring, lifecycle, machine output, and error
branching.

## Install

**Release:**

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh
stackless --version   # expect 0.2.2 or newer
```

**From source (this repo):**

```bash
mise install          # pins Rust 1.96.0 + tooling
cargo build --release # binary: target/release/stackless
export PATH="$PWD/target/release:$PATH"
```

Always prefer `--json` for automation.

## Authoring on-ramp

1. **Greenfield:** `stackless init` scaffolds a minimal valid `stackless.toml`
   (static single-service site, `python3 -m http.server` locally). Use
   `--name <dns-safe>` and `--file <path>`; `--force` overwrites.
2. **Existing repo:** `stackless adopt` inspects `package.json`, `Cargo.toml`,
   `index.html`, etc. and writes or merges a draft definition. Use `--merge`
   to append detected services; always follow with `stackless check`.
3. **Iterate:** `stackless check stackless.toml --on local` (add `--on render`,
   `--on vercel`, etc. for each cloud target). Fix every reported code before
   `up`.
4. **Preflight:** `stackless doctor` (optionally `--file stackless.toml --on
   render`) before first `up` — daemon, persistence, `.stackless.env`,
   cloud API keys, Stripe CLI + Projects plugin.

Read [docs/SCHEMA.md](../../docs/SCHEMA.md) for the full schema: services need
`source`, `health`, and `[services.<name>.local]` with a `run` command binding
`$PORT` on `127.0.0.1`; express dependencies via `env` references, never
`depends_on`.

## Full lifecycle (`--json`)

Run in order for a new stack:

```bash
stackless check stackless.toml --on local --json
stackless doctor --file stackless.toml --json
stackless up --name demo --on local --json
stackless status demo --json
stackless verify demo --json          # requires [stack.verify]; optional --tier smoke
stackless logs demo --tail 100 --json
stackless down demo --json
```

**Resume:** `stackless up --name demo --json` on an existing instance ignores
`--on` (substrate was fixed at creation).

**Cloud:** add `--on render` / `--on vercel` / `--on fly` / `--on netlify` at
creation; set API keys (see doctor). Paid resources need `--confirm-paid`.

**Local source pins:** `stackless up --name demo --on local --source web=. --json`
(cloud substrates reject `--source`).

**Typed bindings:** after the TOML stabilizes, generate a checked-in IDL and
language projections (`Origins`/`bindOrigins`, `Integrations`/`bindIntegrations`,
`SECRETS_REQUIRED`, `VerifyTier`). Bind is not a Client. Languages: `rust`,
`typescript` (`ts`), `go`, `python` (`py`).

```bash
stackless bind --file stackless.toml \
  --idl .stackless/stack.idl.json \
  --emit typescript=e2e/stack.gen.ts \
  --emit rust=tests/support/stack_bind.rs \
  --emit go=internal/stack/origins.go \
  --go-package stacklessbind
stackless bind --file stackless.toml \
  --idl .stackless/stack.idl.json \
  --emit typescript=e2e/stack.gen.ts \
  --emit rust=tests/support/stack_bind.rs \
  --check
```

**Language SDKs** (Rust crate + TypeScript/Python/Go packages) share the same
lifecycle verbs. Transport details: `sdks/PROTOCOL.md`. Integration credentials
for out-of-process tests: verify-tier env (preferred off-stdout) or
`up --json` / `UpOutcome.integrations` (do not scrape vault/`state.db`).

## Machine output contract

| Stream | Content |
|--------|---------|
| **stdout** | Final JSON envelope: `{ "ok": true, ... }` or `{ "ok": false, "error": { ... } }` |
| **stderr** | Human prose in non-JSON mode; **NDJSON progress** during `up --json` |

### Success envelopes

Every success verb emits `{ "schema_version": 1, "ok": true, … }` on stdout.

- `check --json`: `{ "ok": true, "stack", "services", "graph" }`
- `up --json`: `{ "schema_version", "ok", "instance", "substrate", "executed", "skipped", "duration_ms", "steps", "origins", "integrations?", "spend?" }` — `integrations` is a nested `{ dns: { output: value } }` object, omitted when empty; contains credentials
- `down --json`: `{ "schema_version", "ok", "instance", "outcome", "spend?" }`
- `verify --json`: `{ "schema_version", "ok", "instance", "tier?", "duration_ms", "exit_status", "log_path", "lease_remaining_secs?" }`
- `status`/`list --json`: `{ "schema_version", "ok", …report fields…, "persistence_warning?" }`
- `logs --json`: `{ "schema_version", "ok", "instance", "services": [{ "service", "source", "lines?", "reason?" }] }`
- `doctor --json`: `{ "ok": true|false, "checks": [{ "check", "ok", "code?", "remediation?" }] }`
- `init`/`adopt --json`: `{ "ok": true, "path", "next": "stackless check ..." }`

NDJSON progress during `up --json` includes `at_epoch_ms` and optional `duration_ms`
per step on stderr.

### Error envelope

```json
{
  "ok": false,
  "error": {
    "schema_version": 1,
    "code": "def.validate.substrate_config_missing",
    "message": "...",
    "step": "optional",
    "instance": "optional",
    "remediation": "concrete fix",
    "context": { "service": "...", "log_tail": "..." }
  }
}
```

**Branch on `error.code` only.** Never parse `message` or `remediation` for control flow.

### `up --json` NDJSON progress (stderr)

One JSON object per plan step:

```json
{
  "schema_version": 1,
  "event": "step_started|step_skipped|step_completed|step_failed",
  "instance": "demo",
  "step": "health:web",
  "kind": "...",
  "node": "...",
  "index": 3,
  "total": 12,
  "code": "optional on step_failed"
}
```

Parse stderr line-by-line as NDJSON during `up`; keep stdout for the final envelope.

## Error-code decision tree

Use this after any failed `--json` command:

```
error.code?
├─ def.parse.syntax
│  └─ Fix TOML syntax; re-run stackless check
├─ def.parse.schema | def.validate.unknown_key
│  └─ Unknown/mistyped field; compare against docs/SCHEMA.md
├─ def.validate.name_invalid
│  └─ Use DNS-safe names: ^[a-z][a-z0-9-]*$, ≤63 chars
├─ def.validate.no_services
│  └─ Add at least one [services.<name>] block
├─ def.validate.depends_on_rejected
│  └─ Remove depends_on; wire via env (${services.X.origin})
├─ def.validate.undeclared_reference | def.validate.reference_syntax
│  └─ Fix ${...} references to declared stack/instance/service/secret names
├─ def.validate.secret_not_required | secrets.unresolved
│  └─ Add keys to [secrets].required and .stackless.env (or export)
├─ def.validate.substrate_config_missing
│  └─ Add [services.<name>.<substrate>] for every service before up --on <substrate>
├─ def.validate.root_origin_conflict
│  └─ Set root_origin = true on at most one service
├─ def.validate.integration_invalid | integration.config.invalid
│  └─ Fix [integrations.*] block; managed providers are global-only (no per-host tables)
├─ integration.host.unsupported
│  └─ Change --on host or integration provider
├─ state.lock.held
│  └─ Wait for the in-flight operation on that instance; retry
├─ state.instance.exists
│  └─ Pick a different --name or stackless down the existing instance
├─ engine.source_override.unsupported
│  └─ Drop --source/--dirty for cloud; commit and push, pin ref in stackless.toml
├─ render.payment.not_confirmed | vercel.payment.not_confirmed | fly.payment.not_confirmed
│  └─ Re-run with --confirm-paid
├─ vercel.api_key.missing | render.api_key.missing
│  └─ Set VERCEL_TOKEN / RENDER_API_KEY in env, .stackless.env, or key file beside stackless.toml
├─ local.health_failed | local.service_died | local.hook_failed
│  └─ Read error.context.log_tail; fix service/hook; stackless up resumes
├─ daemon.unreachable | daemon.spawn_failed
│  └─ stackless daemon ping; ensure state dir writable
├─ stripe.projects.unavailable | stripe.projects.auth
│  └─ Install Stripe CLI + projects plugin; stripe login; stripe projects init
├─ verify.not_declared
│  └─ Add [stack.verify] with a run command
├─ verify.tier_unknown
│  └─ Add [stack.verify.tiers.<name>] or use the default [stack.verify] tier
├─ verify.tier_required
│  └─ Pass `--tier` with one of the declared tier names (no default `[stack.verify].run`)
├─ verify.failed
│  └─ Read error.context.log_tail and log_path; fix the verify script; re-run stackless verify
├─ doctor.checks.failed
│  └─ Re-run stackless doctor --json; fix each check with ok: false
├─ cli.init.exists | cli.adopt.exists
│  └─ Use --force or --merge (adopt) or pick another --file
└─ (other)
   └─ Grep crates/stackless-core/src/fault.rs and substrate codes; follow remediation verbatim
```

## Secrets and env

- Gitignored **`.stackless.env`** next to `stackless.toml` (`KEY=value` lines) overlays
  the vault; required `[secrets].required` keys must resolve before `up`.
- Cloud: `RENDER_API_KEY`, `VERCEL_TOKEN` (or `.render-api-key`, `.vercel-token`).

## Checklist before first `up`

1. `stackless check stackless.toml --on local --json` → `ok: true`
2. `stackless doctor --file stackless.toml --on <target> --json` → all checks `ok: true`
3. Every service: local `run` binds `$PORT` on `127.0.0.1`
4. Exactly one `root_origin = true` for the user-facing web service (local)
5. Cloud targets: every service has a matching `[services.*.<substrate>]` block
