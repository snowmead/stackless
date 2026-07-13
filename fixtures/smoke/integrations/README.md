# Integration live smokes

One `stackless.toml` per catalog integration (67 total). Each provisions a single
`[integrations.*]` resource under `--on local` with a trivial health-gated probe
service.

## Regenerate fixtures

```sh
mise run generate-smoke-fixtures
```

Writes `manifest.json`, per-slug fixtures, and the `smoke-integration-*` mise tasks.

## Run one smoke

```sh
mise run smoke-integration-neon
```

Paid catalog tiers pass `--confirm-paid` automatically (see `manifest.json`).

## Run all smokes

```sh
mise run smoke-integrations-audit   # list missing stripe projects link <vendor>
mise run smoke-integrations         # full manifest, serial per vendor
mise run smoke-integrations-vendor -- --vendor cloudflare
```

Cloudflare provisions are spaced 15 minutes apart (rate limit).

## Pin credential envelopes

After linking a vendor (`stripe projects link <vendor>`):

```sh
mise run discover neon/postgres -- --dir fixtures/smoke/integrations/neon
bash scripts/smoke_rollout_wave.sh 1   # wave 1 from docs/PROVIDER-WAVES.md
```

Reconcile `OUTPUT_FIELDS` in the provider `.rs` when discover output differs,
then re-run the smoke and commit the fixture project anchor.

## Prerequisites

- `STRIPE_API_KEY` in `.stackless.env` or the environment
- Stripe CLI + pinned `projects` plugin (`mise run stripe-coherence` offline gate)
- Account-level provider links for each vendor in `manifest.json`
