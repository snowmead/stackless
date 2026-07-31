# stackless

Ephemeral software stacks: named, leased, isolated, proven, destroyed.
A Rust workspace (edition 2024); `crates/stackless` is the CLI.

## Project docs — read the relevant one before working in that area

- **[ARCHITECTURE.md](ARCHITECTURE.md)** — systems map and lifecycle pipelines
  in numbered sections (with Mermaid); code comments cite it as `§N`. Read
  before touching core / engine / the `Substrate` & integration seams.
- **[VISION.md](VISION.md)** — founding vision: what stackless is and why.
- **[README.md](README.md)** — What / Why / How (agent-first overview).
- **[docs/SCHEMA.md](docs/SCHEMA.md)** — the complete `stackless.toml` schema
  reference (what the parser/validator actually enforce).
- **[docs/SELFTEST.md](docs/SELFTEST.md)** — the two-tier testing strategy
  (hermetic + gated live smoke) and the Stripe Projects plugin snapshot/drift
  framework. Read before changing tests or the smoke setup.
- **[docs/ADDING-A-PROVIDER.md](docs/ADDING-A-PROVIDER.md)** — how to add a
  hosting substrate or a catalog integration: the one-row registration seams,
  the onboarding tooling below, and the hard-won gotchas. Read before adding or
  changing any provider.

## Provider-onboarding tooling (the `xtask` crate; see ADDING-A-PROVIDER.md)

- `mise run catalog <provider>` — list a provider's catalog services + config
  schemas + pricing (offline).
- `mise run discover <reference> -- --dir <project-dir>` — provision a resource
  once into a throwaway environment, dump its real credential output env vars,
  then tear down (live; needs `STRIPE_API_KEY` + a linked project). The catalog
  describes config *input* but not credential *outputs* — this pins them.
- `mise run new-integration <reference>` — scaffold a provider module from the
  catalog schema.
