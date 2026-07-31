# Supported providers

stackless has two provider families. Adding one touches exactly one
registration site plus the provider's own module/crate — see
[docs/ADDING-A-PROVIDER.md](docs/ADDING-A-PROVIDER.md).

| Family | Registration site | Used as |
|---|---|---|
| **Hosting substrate** | [`crates/stackless/src/substrates.rs`](crates/stackless/src/substrates.rs) | `up --on <name>` |
| **Catalog integration** | [`crates/stackless-integrations/src/registry.rs`](crates/stackless-integrations/src/registry.rs) (`register_providers!`) | `[integrations.<name>]` with `provider = "…"` |

This file is regenerated from those registries. To add or remove a
provider, change the code registration — do not edit the lists below by
hand and expect them to stay correct.

---

## Hosting substrates (`--on`)

| `--on` | Crate | Notes |
|---|---|---|
| `local` | `stackless-local` | Host processes + daemon reverse proxy; supports `--source` / `--dirty` |
| `render` | `stackless-render` | Stripe + Render REST: web services / static sites, deploy poll, health, logs |
| `vercel` | `stackless-vercel` | Stripe + Vercel REST: git-backed projects, deploy poll, health |
| `fly` | `stackless-fly` | Stripe + Fly Machines API; v0 **image-only** |
| `netlify` | `stackless-netlify` | Stripe + Netlify REST; v0 **static upload** |
| `railway` | `stackless-railway` | Stripe `railway/hosting`; Phase 1 provision (deploy API later) |
| `cloudflare` | `stackless-cloudflare` | Stripe `cloudflare/workers` host substrate (distinct from Cloudflare *integrations*) |
| `wordpress` | `stackless-wordpress` | Stripe `wordpress.com/site`; Phase 1 provision |
| `laravel-cloud` | `stackless-laravel-cloud` | Stripe `laravel_cloud/application`; Phase 1 provision |
| `gitlab` | `stackless-gitlab` | Stripe `gitlab/project`; Phase 1 provision |

Cloud substrates share the Stripe Projects + provider-API pattern
described in [ARCHITECTURE.md](ARCHITECTURE.md) §4.

---

## Catalog integrations

Each row is a first-class `[integrations.*]` `provider` value and its
Stripe Projects catalog reference. Offline catalog detail:
`mise run catalog <provider>`.

| `provider` | Catalog reference |
|---|---|
| `agentmail` | `agentmail/api` |
| `agentphone` | `agentphone/number` |
| `algolia` | `algolia/application` |
| `amplitude` | `amplitude/analytics` |
| `auth0` | `auth0/client` |
| `base44` | `base44_projects/app` |
| `blaxel-agent-drive` | `blaxel/agent-drive` |
| `blaxel-sandbox` | `blaxel/sandbox` |
| `browserbase` | `browserbase/project` |
| `chatbase` | `chatbase/agent` |
| `chroma` | `chroma/database` |
| `clerk` | `clerk/auth` |
| `clickhouse` | `clickhouse/clickhouse` |
| `clickhouse-postgres` | `clickhouse/postgres` |
| `cloudflare-browser-run` | `cloudflare/browser-run` |
| `cloudflare-d1` | `cloudflare/d1` |
| `cloudflare-hyperdrive` | `cloudflare/hyperdrive` |
| `cloudflare-kv` | `cloudflare/kv` |
| `cloudflare-queues` | `cloudflare/queues` |
| `cloudflare-r2` | `cloudflare/r2:bucket` |
| `cloudflare-workers` | `cloudflare/workers` |
| `cloudflare-workers-ai` | `cloudflare/workers-ai` |
| `composio` | `composio/project` |
| `customerio` | `customerio/workspace` |
| `datadog-observability` | `datadog/observability` |
| `daytona` | `daytona/sandbox` |
| `depot` | `depot/api` |
| `e2b` | `e2b/sandbox` |
| `elevenlabs` | `elevenlabs/tts` |
| `exa` | `exa/api` |
| `firecrawl` | `firecrawl/api` |
| `flyio-mpg` | `flyio/mpg` |
| `flyio-sprite` | `flyio/sprite` |
| `gitlab` | `gitlab/project` |
| `heygen` | `heygen/api` |
| `huggingface` | `huggingface/platform` |
| `huggingface-bucket` | `huggingface/bucket` |
| `inngest` | `inngest/app` |
| `kernel` | `kernel/project` |
| `laravel-cloud` | `laravel_cloud/application` |
| `laravel-cloud-mysql` | `laravel_cloud/mysql` |
| `laravel-cloud-valkey` | `laravel_cloud/valkey` |
| `metronome` | `metronome/sandbox` |
| `mixpanel` | `mixpanel/analytics` |
| `neon` | `neon/postgres` |
| `openrouter` | `openrouter/api` |
| `parallel` | `parallel/api` |
| `planetscale-mysql` | `planetscale/mysql` |
| `planetscale-postgresql` | `planetscale/postgresql` |
| `postalform` | `postalform/mail` |
| `posthog` | `posthog/analytics` |
| `prisma` | `prisma/database` |
| `privy` | `privy/app` |
| `pydantic` | `pydantic/logfire` |
| `railway-bucket` | `railway/bucket` |
| `railway-hosting` | `railway/hosting` |
| `railway-mongo` | `railway/mongo` |
| `railway-postgres` | `railway/postgres` |
| `railway-redis` | `railway/redis` |
| `render-postgres` | `render/postgres` |
| `revenuecat` | `revenuecat/app` |
| `runloop` | `runloop/sandbox` |
| `schematic` | `schematic/schematic-environment` |
| `sentry` | `sentry/project` |
| `sentry-seer` | `sentry/seer` |
| `steel` | `steel/browser` |
| `supabase` | `supabase/project` |
| `supermemory` | `supermemory/memory` |
| `tabstack` | `tabstack/api` |
| `turso` | `turso/database` |
| `twilio-email` | `twilio/email` |
| `upstash-qstash` | `upstash/qstash` |
| `upstash-redis` | `upstash/redis` |
| `upstash-search` | `upstash/search` |
| `upstash-vector` | `upstash/vector` |
| `wix` | `wix/headless` |
| `wordpress-com` | `wordpress.com/site` |
| `workos` | `workos/auth` |
