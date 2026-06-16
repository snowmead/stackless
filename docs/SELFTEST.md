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

## Tier 2 — live smoke (gated; nightly / on-demand)

A real `up` → health → `down` → verified-gone against the actual cloud, with **no
second repo**: the smoke fixtures deploy *this repo's own source*.

```
fixtures/smoke/
  site/index.html        # deployable static page (marker: stackless-smoke-ok)
  vercel/stackless.toml   # deploy = "upload" (uploads fixtures/smoke/site)
  render/stackless.toml   # render static site publishing fixtures/smoke/site
```

Run locally (reads creds from `.stackless.env`):

```
mise run smoke-vercel     # up smoke-v --on vercel, then down, fail if either fails
mise run smoke-render
mise run smoke            # both
```

In CI: `.github/workflows/smoke.yml` (`workflow_dispatch` + nightly), one gated
job per provider, secrets `STRIPE_API_KEY` / `VERCEL_TOKEN` / `RENDER_API_KEY`.

### Prerequisites (one-time, human)

- The Stripe Project must have the provider **linked**: `stripe projects link
  vercel` / `render` (account-level; new projects inherit it). Check with
  `stripe projects status`.
- The provider API token must belong to the **linked** account/team. For Vercel,
  the substrate already reads the Stripe-managed token + `VERCEL_ORG_ID` from the
  instance env — see the **Vercel notes** at the end of this doc.
- **Vercel git-source mode** additionally needs the Stripe-managed Vercel team
  connected to GitHub with access to the repo (else `git_info_fail`). The default
  `deploy = "upload"` avoids this entirely.

### Adding a provider/integration

1. Implement the `Substrate` (or `Hostable`) as usual.
2. Add Tier-1 hermetic tests (mock the provider API + Stripe runner).
3. Drop a `fixtures/smoke/<name>/stackless.toml` that deploys `fixtures/smoke/site`.
4. Add a `mise run smoke-<name>` task and a matrix entry in `smoke.yml`.

## Vercel notes (hard-won, verified live)

- Stripe provisions Vercel projects in its **own managed team**; use the
  `VERCEL_TOKEN` + `VERCEL_ORG_ID` it puts in the instance env (not your token).
- Deployments need `skipAutoDetectionConfirmation=1` (raw POST, not the generated
  client).
- stackless **disables deployment protection** per project so the stack is
  publicly health-checkable.
- `deploy = "git" | "upload"`: `upload` clones the ref and posts inline files
  (no Vercel↔GitHub connection); `git` needs the team connected to GitHub.
