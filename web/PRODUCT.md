# Product

## Register

brand

## Users

AI agents (and the engineers who hand them tools) that need to test changes against real ephemeral infrastructure. Context: many named, isolated stacks in parallel; environments must be creatable, verifiable, and destroyable without leftover cost or state.

## Product Purpose

stackless is the lifecycle contract between an agent and a disposable stack: one `stackless.toml`, then `up` / `verify` / `down` (or lease expiry). Not a PaaS, not IaC state management, not a production orchestrator. Success for this site: understand the contract in seconds, install once, hand the rest to an agent.

## Brand Personality

Voice: terse, technical, agent-first. Tone: explain the lifecycle contract; don't sell vibes. Feel: competence through clarity (what exists, what dies, what proves). No warmth theater, no precision cosplay.

## Anti-references

- Cream paper + terracotta / copper SaaS landings
- Purple AI glow and neon-on-black "infra" tropes
- Identical icon + heading + blurb card grids
- Chatty CLI marketing and hero-metric SaaS templates

## Design Principles

1. **Show the contract** — Prefer toml, verbs, and JSON envelopes over metaphor.
2. **Agent-first, human-brief** — Humans get a short path to install; machines get the real interface.
3. **Death is a feature** — Teardown, leases, and verified absence are first-class, not footnotes.
4. **Same story, sharper craft** — Keep the narrative; replace the visual identity and tighten structure.
5. **No vibes, only proof** — Silence is not success; provisioned is not verified.

## Accessibility & Inclusion

WCAG AA. Honor `prefers-reduced-motion`. No stricter requirements beyond that.
