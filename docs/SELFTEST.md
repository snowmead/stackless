# Self-testing providers & integrations

Every provider (substrate) and integration stackless drives through Stripe
Projects is exercised by a real `up`/`down`, in two tiers:

## Tier 1 — hermetic (every CI run, no secrets, no network)

`#[cfg(test)]` tests in each crate drive the provider/integration logic against
`wiremock` (the provider HTTP API) and a mocked Stripe `CommandRunner`. They run
in `cargo nextest run --workspace` — i.e. the `test` job in `ci.yml`, on every
PR. This is the gate that must stay green.

Examples: `crates/stackless-vercel/src/vercel_api.rs` (git/upload deploy +
protection), `…/lib.rs` (`observe`), `crates/stackless-integrations/src/providers/clerk.rs`
(provision → observe → destroy).

## Tier 2 — live smoke (gated; every PR on the canonical repo)

A real `up` → health → `status`/`logs` → `down` → verified-gone against the
actual cloud, with **no second repo**: the smoke fixtures deploy *this repo's
own source*. Every verb runs `--json` and the runner asserts each stdout
envelope parses with `ok: true`, so the machine contract (including the
per-substrate `logs` sources and `spend` fields) gets live coverage, not just
the hermetic wiremock tier.

On `snowmead/stackless`, these run on **every PR** via `ci.yml` (`live-smoke`
and `live-fleet` jobs). Forks skip them (no secrets). `smoke.yml` repeats the
same matrix nightly and on `workflow_dispatch` as a scheduled safety net.

```
fixtures/smoke/
  site/index.html        # deployable static page (marker: stackless-smoke-ok)
  vercel/stackless.toml   # deploy = "upload" (uploads fixtures/smoke/site)
  render/stackless.toml   # render static site publishing fixtures/smoke/site
  flyio/stackless.toml    # Fly Machines image deploy (paid; --confirm-paid)
  netlify/stackless.toml  # Netlify file-digest static upload
  cloudflare/stackless.toml  # deprecated combined fixture; use integrations/
  integrations/            # one stackless.toml per catalog integration (67)
```

Substrate smokes run on **every PR** via `ci.yml` (`live-smoke`). Catalog
integration smokes (67 fixtures under `fixtures/smoke/integrations/`) run
**nightly** via `smoke.yml` `smoke-integrations` (vendor-sharded matrix).

Run locally (reads creds from `.stackless.env`):

```
mise run smoke-vercel     # up smoke-v --on vercel, then down, fail if either fails
mise run smoke-render
mise run smoke-fly        # paid Fly app — needs --confirm-paid in the fixture runner
mise run smoke-netlify
mise run smoke-cloudflare # all cloudflare integration smokes (serial, spaced)
mise run smoke            # substrate smokes only (see mise.toml)
mise run smoke-integration-neon   # one catalog integration
mise run smoke-integrations       # full integration manifest
mise run smoke-integrations-audit # stripe projects link audit
```

In CI: `ci.yml` runs one gated job per **substrate** on every PR to `main` (plus
`live-fleet` for Turso). `.github/workflows/smoke.yml` repeats the substrate
matrix nightly and runs the full **integration** vendor matrix (`smoke-integrations`
job). Secrets: `STRIPE_API_KEY` / `VERCEL_TOKEN` / `RENDER_API_KEY` /
`FLY_API_TOKEN` / `STACKLESS_STATE_URL` / `STACKLESS_STATE_TOKEN`.

### Prerequisites (one-time, human)

- The Stripe account must have each integration vendor **linked**:
  `mise run smoke-integrations-audit` lists missing `stripe projects link <vendor>`
  steps (account-level; interactive OAuth).
- The provider API token must belong to the **linked** account/team. For Vercel,
  the substrate already reads the Stripe-managed token + `VERCEL_ORG_ID` from the
  instance env — see the **Vercel notes** at the end of this doc.
- **Vercel git-source mode** additionally needs the Stripe-managed Vercel team
  connected to GitHub with access to the repo (else `git_info_fail`). The default
  `deploy = "upload"` avoids this entirely.

### Adding a provider/integration

1. Implement the `Substrate` (or `Hostable` + `ProviderOps`) — see
   `docs/ADDING-A-PROVIDER.md` for the full checklist.
2. Add Tier-1 hermetic tests (mock the provider API + Stripe runner) and a
   catalog-gap test (`verify_service`) per config.
3. Regenerate or add a fixture under `fixtures/smoke/integrations/<slug>/` via
   `mise run generate-smoke-fixtures` (catalog integrations), or
   `fixtures/smoke/<name>/stackless.toml` for a **substrate** (deploys
   `fixtures/smoke/site`). Integrations run `--on local` with a trivial probe
   service (see `fixtures/smoke/integrations/README.md`). The first live run
   pins the credential output envelope — reconcile `OUTPUT_FIELDS` with
   `mise run discover <reference>`.
4. Add a `mise run smoke-integration-<slug>` task (generated) or substrate
   `smoke-<name>` task and a matrix entry in `smoke.yml`.

## Stripe Projects plugin snapshots (versioned, auto-watched)

The `stripe projects` plugin is versioned independently of the Stripe CLI, ships
no changelog, and its catalog is server-side. Three committed artifacts under
`crates/stackless-stripe-projects/tests/fixtures/` make upgrades reproducible and
turn their `git diff` into the changelog Stripe never publishes:

- `plugin-version.txt` — the pinned plugin version (the version of record; CI
  installs exactly this).
- `command-surface.txt` — every `stripe projects` subcommand's `--help`, so
  added/removed/renamed commands and flags show up as a line diff.
- `catalog.json` — the provider catalog (services, schemas, pricing); the typed
  model in `src/catalog.rs` must fully cover it.

All three are regenerated by one bless path — the `refresh_blesses_snapshots`
test, gated on `STRIPE_PROJECTS_REFRESH=1`, which refuses to write if the live
catalog has wire-format the model does not cover. It is never run by hand; the
infrastructure drives it:

- **Hermetic (every PR):** `fixtures_are_coherent` (runs in `mise run test`)
  checks the artifacts agree offline — surface header == pinned version, blocks
  == the code's `TRAVERSAL`, every banner command is captured — plus the local
  `prek` pre-commit/pre-push hooks (auto-wired by `mise install`).
- **Live pinned gate (nightly smoke):** `smoke.yml` installs the pinned plugin,
  re-blesses, and fails if `command-surface.txt` / `plugin-version.txt` drift or
  the live catalog is unmodeled.
- **Watcher → auto-PR (nightly):** `stripe-projects-watch.yml` installs the
  *latest* plugin and opens a PR with the regenerated fixtures whenever upstream
  changes — the only human step is reviewing it.

To upgrade by hand:

```
stripe plugin install projects@<version>                     # pin
mise run stripe-refresh                                      # re-bless the three artifacts
git diff crates/stackless-stripe-projects/tests/fixtures/    # the changelog
# if the bless fails, add the new variant/field to src/catalog.rs and re-run
```

## Vercel notes (hard-won, verified live)

- Stripe provisions Vercel projects in its **own managed team**; use the
  `VERCEL_TOKEN` + `VERCEL_ORG_ID` it puts in the instance env (not your token).
- Deployments need `skipAutoDetectionConfirmation=1` (raw POST, not the generated
  client).
- stackless **disables deployment protection** per project so the stack is
  publicly health-checkable.
- `deploy = "git" | "upload"`: `upload` clones the ref and posts inline files
  (no Vercel↔GitHub connection); `git` needs the team connected to GitHub.
