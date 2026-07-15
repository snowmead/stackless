# Adding a provider

For parallel catalog rollouts (waves, merge gates, exclusions), see
[PROVIDER-WAVES.md](PROVIDER-WAVES.md).

stackless has two provider families. Adding one touches **exactly one
registration site** plus the provider's own module/crate — the engine, core, and
sibling providers stay untouched (core never names a provider).

- **Catalog integration** (auth, object storage, db, queue, …) — a resource
  provisioned through Stripe Projects. Most of the catalog is this kind.
- **Cloud substrate** (Render/Vercel-shaped: a hosting target for `--on`) — runs
  the lifecycle engine against a cloud REST API.

Everything is provisioned through the Stripe Projects catalog, so each resource
gets config-schema validation, paid-confirmation, project-environment isolation,
and spend-cap semantics for free. A service **not** in the catalog is out of
scope (it would need a separate provisioning authority).

Provider-facing traits and helpers live in **`stackless-provider-sdk`**
(`crates/stackless-provider-sdk`). Implementors depend on that crate (or use the
re-exports from `stackless-integrations` when adding an in-tree provider).

---

## Tooling: start here

```sh
mise run catalog <provider>          # list a provider's services: schema, pricing, paid?
mise run discover <reference> -- --dir fixtures/smoke/cloudflare   # provision once, dump the real output env vars, tear down
```

`catalog` is offline (reads the committed catalog fixture). `discover` is live
(needs a linked Stripe project + the provider linked — see next section) — it pins
the credential **output envelope**, which the catalog does *not* describe. Both are
the `xtask` crate.

## One-time setup: initialize + link the provider (do this before `discover`/smoke)

`discover` and the live smoke both drive the `stripe projects` plugin, which only
operates **inside an initialized Stripe-project directory** (one that has a
`.projects/` dir). Each smoke fixture is its own such directory. So before the
first `discover` or smoke of a **new** provider, do this once, from that fixture
dir:

```sh
cd fixtures/smoke/<name>
# Creates .projects/ — exactly what `stackless up` runs via ensure_project.
stripe projects init <stack-name> --skip-skills --accept-tos
# Interactive provider OAuth — must be a human; opens a browser/device flow.
stripe projects link <provider>
```

- **The `link` command needs a project context**, which is why `init` comes first.
  Running any `stripe projects` verb from a *non*-initialized dir just prints the
  "Get started by running `stripe projects init`" welcome and does nothing — that
  is the symptom of skipping `init`.
- **Provider links are account-level**, not per-project: once linked, the provider
  shows on *every* project (`stripe projects status` lists them, e.g.
  `Providers ✓ Cloudflare, ✓ Vercel, ✓ Render`). So you only link each provider
  once per Stripe account, ever — but from inside *some* initialized project dir.
- The `<provider>` slug is the lowercased catalog provider name (`cloudflare`,
  `flyio`, `render`, `vercel`, …). Re-link a stale provider
  (`PENDING_AUTH`/`EXPIRED`) with `stripe projects link --force <provider>`.
- Paid providers also need a billing method on the account
  (`stripe projects billing add`); confirm the account with `stripe projects status`.
- `stackless up` runs `init` for you on first deploy and records the project id
  into the fixture's `[stack.projects.stripe].project`; the `.projects/` runtime
  state is gitignored. So the genuinely manual, can't-automate step is just the
  one-time `link`.

## Catalog integration (the common case)

A resource whose credentials come back as flat env vars (Cloudflare R2/KV/D1/…)
is a `CatalogResource`. The provision/observe/destroy lifecycle and credential
resolution are shared — you only declare config + fields. One file under
`crates/stackless-integrations/src/providers/<provider>/<service>.rs` (using
traits from `stackless-provider-sdk`):

1. **Config** + `impl CatalogService { const REFERENCE = "<provider>/<service>" }`
   — the `stripe projects add` key (note: `provider_name.lowercase()/service_id`).
   The `Serialize` shape is validated against the catalog schema at provision.
2. **`impl Hostable`** — `PROVIDER` (the `provider = "..."` key, **distinct per
   service**, e.g. `"cloudflare-r2"`), `HOSTING` (`Managed`), `CONFIG_SCOPE`
   (`GlobalOnly`), `RESOURCE_KIND` (unique), `OUTPUTS`. *Optional:*
   `BLOCKED_SETTINGS` — config keys the credentials can't toggle (no secret-key
   endpoint); each `(key, remediation)` fails `check`/`up` loudly instead of
   being silently ignored. Defaults to empty; see `providers/clerk.rs` and
   docs/DECISIONS.md.
3. **`impl CatalogResource`** — `type Config`, `PROVIDER_PREFIX`
   (the unambiguous env-var prefix, e.g. `"CLOUDFLARE"`), `OUTPUT_FIELDS`
   (`(env-suffix, output, required)` — get the real suffixes from `discover`),
   and `build_config`. **`ProviderOps` is derived** by a blanket impl — no
   per-service lifecycle code. Post-provision vendor glue (if any) goes in
   `ProviderOps::apply` (default no-op).
4. **`validate_config`** — provider-specific config checks beyond the schema.

Then the **one registration site** — one row in `register_providers!`
(`crates/stackless-integrations/src/registry.rs`) + a `pub mod` in the provider's
`mod.rs`. Dispatch is automatic (`ops_for` / `ops_for_resource_kind`); never a
provider string. (See `providers/cloudflare/r2.rs` for a worked example.)

**Tests** (both offline, every CI run):
- Catalog-gap: `verify_service(&catalog, &<sample config>)` against the committed
  `catalog.json` — fails if the reference is absent or the schema drifted.
- Provision: `test_support::provision_script(<catalog envelope>, json!({<vars>}))`
  builds the whole CLI conversation; the test is ~5 lines (see `cloudflare/kv.rs`).

## Bespoke integration (single credential blob)

If credentials arrive as one provider-specific JSON blob (Clerk), implement
`ProviderOps` directly and parse the blob yourself — see `providers/clerk.rs`
(`provision_with_credentials` + `parse_clerk_credentials`). Post-provision vendor
API toggles belong in `ProviderOps::apply` (see Clerk's `organizations` toggle).
`observe()` returns `IntegrationObservation` (Stripe registration plus optional
`Drift` entries); the dispatch layer reduces it to `Present`/`Gone` for the
engine today. Everything else (registry row, gap test, hermetic test) is the
same.

## Cloud substrate

1. **New crate** `crates/stackless-<name>` depending on `stackless-cloud`; add to
   workspace `members` + `[workspace.dependencies]` + the binary's deps.
2. **`pub const SUBSTRATE_NAME`** (the `--on` name).
3. **`impl Substrate`** (`crates/stackless-core/src/substrate.rs`) — `execute`,
   `observe`, `destroy`, and default-overridable `spend_line` / `fetch_logs`.
   `#[async_trait]` (the trait is used as `dyn`).
4. **Reuse `stackless-cloud`**: `credential::resolve(ENV, FILE, …)` and
   `prepare::run_prepare_command(…)`; map their neutral failures to your errors.
5. **Own `error.rs` + `codes.rs`** (`<NAME>_*` + `pub const ALL`); add your
   `codes::ALL` to the workspace uniqueness test in `stackless/src/substrates.rs`.
6. **Generated REST client** (if needed): vendor the spec under `specs/`, add a
   block to `specs/regen-clients.sh`, `mise run regen-clients`.

Then one `SUBSTRATES` row in `crates/stackless/src/substrates.rs`. The deeper
cloud lifecycle (deploy polling, health gating) is deliberately **not** shared —
it differs materially between providers (extract only at a third substrate).

## Smoke

A live smoke runs through the shared `fixtures/smoke/run.sh` (always tears down,
unique per-run instance name). Add `fixtures/smoke/<name>/stackless.toml`, a
`smoke-<name>` mise task calling `run.sh`, and a `smoke.yml` matrix entry.
Catalog integrations have no deploy target, so they smoke under `--on local`
with a trivial probe service (see `fixtures/smoke/cloudflare`). Before a new
provider's first smoke, do the one-time `stripe projects init` + `link` from its
fixture dir (see "One-time setup" above) — the smoke fails at the first
`stripe projects add` otherwise.

---

## Gotchas (learned the hard way)

- **The catalog has config schema but no *output* schema.** You cannot know the
  credential env-var names without provisioning — use `mise run discover`.
- **Output env-var names are dynamic.** Stripe names them `{RESOURCE}_{SUFFIX}`
  when several resources share an environment, or `{PROVIDER}_{SUFFIX}` when
  unambiguous. The shared resolution tries both — declare just the `SUFFIX` in
  `OUTPUT_FIELDS`. Outputs must be flat strings (interpolation ignores non-strings).
- **Paid auto-confirms within the spend cap.** `add_resource` passes
  `--confirm-paid-service` automatically when the catalog tier is paid (R2). A
  service whose catalog pricing is `component`/"unavailable" but that demands
  confirmation live (`PRICE_CONFIRMATION_REQUIRED`, e.g. `cloudflare/containers`)
  has **unknown cost** and is **not** auto-provisioned — excluded by default.
- **Providers rate-limit provisioning.** Cloudflare allows ~2 provisions per
  ~22-min window. So a smoke can't bring up many resources at once, and envelope
  discovery is spaced. Verification is gap + hermetic tests (offline, always) +
  spaced live pinning — not "everything live in one run".
- **Teardown removes the Stripe *record*, not always the provider resource.**
  `down` reports "verified gone" via the Stripe registration; the underlying
  provider resource may linger (e.g. a Cloudflare namespace), which is why smokes
  use unique names. Don't assume re-provisioning the same name is collision-free.
- **Not everything is disposable.** `registrar:domain` is a one-time non-refundable
  domain purchase — never smoke it.
