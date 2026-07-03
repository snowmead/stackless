# Decisions

Architecture decisions and the reasoning behind them. Newest first.

---

## 2. Extract `stackless-provider-sdk` as the provider extension surface

**Date:** 2026-07-02

**Decision.** Provider-facing traits (`Hostable`, `ProviderOps`, `CatalogResource`),
errors, config helpers, and observation types live in a dedicated
`stackless-provider-sdk` crate. In-tree providers register via
`register_providers!` in `stackless-integrations`; the monorepo remains the
registration point (no dynamic loading yet).

**What changed.**
- **`ProviderOps::apply`** — default no-op post-provision hook for vendor API
  glue. Clerk's `organizations` toggle moved here from inline provision code.
- **`IntegrationObservation`** — structured observe result (`Present { drift }` /
  `Gone`). Providers return it; the dispatch layer reduces to core `Observation`
  until a check-time drift surface consumes the extra data.
- **`register_providers!`** — one macro row per provider replaces the hand-written
  `PROVIDERS` table.

**Rationale.** Makes the extension API deliberate and separable from dispatch/
validation orchestration, without changing the Stripe Projects scope or requiring
out-of-tree plugin loading.

**Known follow-up.** Follow-up #3 (unobserved post-provision toggles) is
partially addressed: the observation type can carry drift, but providers still
return empty drift and the engine still sees only `Present`/`Gone`.

---

## 1. stackless wraps Stripe Projects; it does not adopt a general IaC engine

**Date:** 2026-06-22

**Decision.** stackless provisions cloud resources by delegating to Stripe
Projects (the projects.dev catalog) and owns the ephemeral-lifecycle engine on
top. It does **not** adopt Terraform, OpenTofu, Pulumi, or Crossplane.

**Context.** An agent tried to enable Clerk *username* sign-in during a bring-up
and couldn't. Clerk's secret-key Backend API has no endpoint to toggle sign-in
identifiers — that setting lives only in the Dashboard, the browser-session
Frontend API, or a private-beta Platform token. `organizations = true` works
only because Clerk shipped a dedicated secret-key endpoint for it. The failure
was a *vendor API gap*, and it was silent: free-form TOML accepts an unknown key
like `username = true` and ignores it. This raised the question of whether
stackless is reimplementing Terraform and should wrap an existing IaC engine.

**Why not adopt an IaC engine.**
- Provisioning is already delegated to Stripe Projects — that *is* the wrapper
  layer. ~95% of per-provider provisioning code is generic (the Cloudflare
  providers are 60–75 lines each).
- What stackless owns on top — the wiring DAG, checkpoint journal, leases/TTL,
  observe-on-resume, spend accounting, teardown-verification — is the
  *disposable-stack* lifecycle that IaC engines don't provide. That is the value.
- An IaC engine would not have solved the Clerk problem: its providers wrap the
  same vendor API, so a missing endpoint stays missing.
- It would introduce a **second state model** (the engine's state graph)
  competing with the checkpoint journal, violating the "one name, one truth"
  invariant — a pivot, not an additive step.

The real recurring cost is per-provider **post-provision config glue** (toggling
SaaS features via bespoke APIs). No IaC engine eliminates this; each ships
hand-written per-provider plugins. Clerk is the only provider with any such glue
today.

**What we did instead.** Per-provider `Hostable::BLOCKED_SETTINGS` declarations
that fail `check`/`up` validation with a precise out-of-band remediation when a
non-toggleable setting is requested — so the failure is loud and early instead
of silent. This is per-provider data, not an execution engine. Post-provision
vendor glue uses `ProviderOps::apply` (see decision #2).

**Revisit if** provider CRUD (not post-provision glue) becomes the dominant
maintenance cost, or Stripe-catalog drift forces a richer provider model.

**Known follow-ups (not addressed here).**
1. **Catalog output-envelope drift.** The catalog has config schema but no output
   schema; env envelopes are hand-pinned via `mise run discover` and break
   silently if Stripe's envelope changes.
2. **Teardown orphan risk.** Removing the Stripe record can leave provider-side
   resources alive (e.g. `clerk::destroy` removes only the Stripe resource).
3. **Unobserved post-provision toggles.** `observe()` checks only Stripe
   registration, so a checkpoint can read `organizations = true / done` while the
   provider-side setting is unknown. Worth an observe/verify story before
   post-provision glue grows.
