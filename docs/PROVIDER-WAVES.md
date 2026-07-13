# Provider wave protocol

How we land Stripe Projects catalog integrations in parallel without merge hell.
See [ADDING-A-PROVIDER.md](ADDING-A-PROVIDER.md) for the per-provider code pattern.

## Phase 1 vs Phase 2

- **Phase 1** — Catalog integrations (`CatalogResource` + `register_providers!`).
  One PR per provider family. Hosting-shaped refs (`railway/hosting`,
  `gitlab/project`, …) are integrations first; they do **not** add `--on`.
- **Phase 2** — Cloud substrates (`--on railway`, gitlab, laravel cloud,
  wordpress.com, cloudflare). Serialized; live smoke required.

## Exclusions

Never auto-provision or smoke:

- `cloudflare/containers`, `cloudflare/registrar:domain`
- `cloudflare/workers:free`, `cloudflare/workers:paid` (plans, not integrations)
- `squarespace/domain`, `wordpress.com/domain` (non-refundable domain purchase)

Plan-tier catalog entries (`*/hobby`, `*/pro`, …) are not adapters.

## Fixed-base wave

1. Pick a wave base commit on `main` (or the previous landed wave tip).
2. For each family in the wave, branch `feat/integration-<family>` from that
   **same** base — no mid-wave rebases onto each other.
3. Each family PR contains only:
   - `crates/stackless-integrations/src/providers/<family>/…`
   - its `register_providers!` row(s) and `pub mod` declarations
   - gap + hermetic tests
   - README/SCHEMA checklist tick for that family
4. Keep `mise.toml` / `.github/workflows/smoke.yml` out of family PRs unless
   regenerating integration smoke fixtures for the landed wave (`mise run
   generate-smoke-fixtures`).

## Landing

Either:

- **Serial merge:** merge family PRs one-by-one; resolve the single additive
  conflict in `registry.rs` / `providers/mod.rs` (union-sort rows).
- **Landing branch:** cherry-pick approved family commits onto `wave-N`,
  union-sort registry rows, run `mise run check` +
  `cargo nextest run -p stackless-integrations`, then one PR to `main`.

Do not invent codegen solely to avoid one-line registry conflicts.

## Merge gate (Phase 1 family PR)

1. Config + `CatalogService::REFERENCE` matches catalog schema (gap test).
2. `Hostable` + `CatalogResource` (or Clerk-shaped bespoke `ProviderOps`).
3. Registry row(s) + uniqueness tests green.
4. Hermetic provision test via `provision_script`.
5. `OUTPUT_FIELDS` pinned by live `mise run discover <ref>` + passing
   `mise run smoke-integration-<slug>` (fixture under `fixtures/smoke/integrations/`).
6. Live smoke fixture present and listed in `fixtures/smoke/integrations/manifest.json`
   (required before wave landing; nightly CI runs vendor-sharded jobs).

## Suggested waves

| Wave | Families |
|------|----------|
| 1 | Neon, Supabase, Turso, Upstash, Auth0, WorkOS, Privy, Prisma |
| 2 | PlanetScale, ClickHouse, Chroma, Sentry, PostHog, Amplitude, Mixpanel, Algolia |
| 3 | OpenRouter, Exa, Firecrawl, Parallel, ElevenLabs, HeyGen, HuggingFace, Inngest |
| 4 | E2B, Daytona, Browserbase, Blaxel, Runloop, KERNEL, AgentMail, AgentPhone |
| 5 | Railway, GitLab, Laravel_Cloud, WordPress.com (site), Base44_Projects, Wix, PostalForm, Metronome, Supermemory |
| 6 | Render (`render/postgres`), Flyio (`flyio/mpg`, `flyio/sprite`) |
