# stackless — founding vision

## What this is

stackless is a CLI that owns the complete lifecycle of disposable software
stacks. It spawns a full, isolated, working copy of a product — every
service, database, and credential it needs — gives it a name, proves it
works, guarantees its death when it is no longer needed, and ensures the
operator never sees a single real secret along the way. On a laptop or on
a cloud provider, for a human, a CI job, or — first and foremost — an AI
agent.

The unit of thought is the **stack instance**: a named, short-lived,
fully-wired incarnation of an application, born from one command and one
declarative definition, sealed behind a secret-blind boundary, gone
without residue from one command or one expired lease.

## Who it is for

stackless is built for autonomous agents first and humans second. Every
design tension resolves in favor of what a fleet of untrusted, tireless,
fallible agents needs: no interactive prompts, no tribal knowledge, no
trust extended that isn't enforced, no secret an agent could read even if
it tried, no cost left running because something forgot to clean up. A
human at a terminal is a welcome guest in an interface shaped for machines.

## Why it should exist

AI agents changed the economics of environments. Where a team once needed
one dev setup and one staging, a fleet of agents needs dozens of
simultaneous, isolated, verifiable stacks per day — each cheap to create,
impossible to confuse with its siblings, safe to abandon, and safe to hand
code you don't trust. Today that capability is assembled by hand from
fragments that each own one layer and none of the whole: container tools
run local services but guarantee nothing about identity, verification, or
cleanup. Infrastructure-as-code owns state but not ephemerality. Provider
blueprints own one vendor. Provisioning CLIs create resources but can't
configure, verify, or reliably destroy them. Egress firewalls contain
untrusted code but know nothing about the stack they protect. Every team
wiring agents into real testing rebuilds the same glue — collision-proof
naming, port juggling, credential plumbing, teardown verification, cost
hygiene, secret isolation — and every team rediscovers the same failure
modes the hard way.

stackless is that glue, productized: the missing lifecycle-and-trust layer
between an agent and the stack it works on.

## The stance

**Unopinionated about the application. Opinionated about the lifecycle.
Uncompromising about secrets.**

stackless never cares what your services are — any language, any runtime,
any command, any wiring between them. It is ruthlessly opinionated about
how instances live: named, leased, isolated, proven, accounted for,
destroyed. And it is absolute about one thing above all: the operator
driving stackless — the agent — never holds a real credential and can
never reach the network except where the stack itself declares. That
asymmetry is the product.

## The trust boundary

An agent is untrusted code with a credit card and a shell. stackless treats
it that way. Every stack instance is born inside a boundary that sits
between the agent (and the stack's own services) and everything else:

- **Durable secrets live only inside the boundary.** Neither the agent
  nor the services it can edit and run ever hold a real third-party
  credential. They hold opaque tokens; the boundary swaps in the real
  value at the moment a request leaves. Secrets minted for the instance
  itself — a database password, a per-instance key — are protected by
  leasing instead: they die with the instance and are useless outside
  it, so blinding them buys nothing (ARCHITECTURE.md §5). An agent can
  read every file, every token, every line of config, edit the server to
  print its entire environment — and exfiltrate nothing durable.
- **Egress is default-deny, and the allowlist is the stack definition.**
  stackless already knows every origin an instance contains and every third
  party it declares; from that it derives exactly where the instance may
  reach. Everything else is refused. A poisoned dependency or a prompt
  injection cannot phone home, open a reverse shell, or reach a metadata
  endpoint, because the stack never said it could.
- **The boundary is born with the instance and dies with it.** Its policy —
  which tokens map to which secrets, which origins are reachable — is
  generated per instance from the same definition that spawns the services.
  There is no separate firewall to configure, no allowlist to hand-maintain,
  no secret store for the agent to discover.

This is not a bolt-on security mode, but it is sequenced after the
lifecycle layer: v0 ships without it, on operator-visible, test-scoped
secrets, keeping the seam that makes the boundary an addition rather
than a redesign (ARCHITECTURE.md §0). Once the boundary ships, an
instance without it is the rare, loud, deliberate exception — never the
default.

## What done looks like

- One definition file describes the stack once: its pieces, how they wire
  into each other, what secrets they need, what third parties they reach,
  and what "healthy" means. The same definition spawns locally and on a
  cloud provider; only what genuinely differs per substrate is expressed
  per substrate.
- One verb creates a named instance; one verb destroys it; one verb proves
  it works. Proof is first-class: an instance isn't "up" because its
  processes started — it's up when its health contract passes.
- Any number of instances of the same stack coexist on one machine or one
  provider account without sharing or colliding on anything: not ports,
  not names, not data, not credentials, not network policy.
- Every instance carries a lease. When the lease expires, the instance is
  reaped — teardown is a system guarantee, not a memory burden on whoever
  or whatever spawned it.
- Teardown is verified, not assumed: after destruction stackless checks the
  substrate for survivors and refuses to report success while anything that
  costs money or holds state remains.
- The agent never sees a secret, and cannot leave the network except where
  the stack allows — by construction, not by policy it could edit around.
- The entire surface is equally usable by a person, a CI job, and an
  autonomous agent: non-interactive by default, structured output always
  available, every operation resumable after interruption, every error
  stating what to do next. Spending money always requires explicit,
  per-invocation consent.

## Invariants (laws, not designs)

1. **One name, one truth.** An instance has exactly one identity, assigned
   at birth and persisted — everything it owns is derived from that name,
   and nothing is re-derived from ambient context that could drift.
2. **Provisioned ≠ configured ≠ verified.** Creating resources, wiring
   them, and proving them are distinct obligations; reporting success at
   any stage without completing the later ones is a lie.
3. **Resume, don't duplicate.** Any interrupted operation can be re-run; the
   system reconciles against what actually exists rather than creating
   siblings or starting over.
4. **Silence is not success.** Cleanup, health, and cost are confirmed by
   observation, never inferred from the absence of errors.
5. **The operator is blind to durable secrets.** Real third-party
   credentials exist only inside the instance's boundary. The agent and
   the stack's own services hold tokens, never values; the boundary
   injects the real secret at egress. Instance-minted credentials are
   exempt: leasing already makes them worthless outside the instance
   (invariant 6). No durable secret is ever written anywhere the
   operator can read it.
6. **Secrets are leased.** Credentials minted for an instance live exactly
   as long as it does, are scoped to it alone, and are never durable or
   shared. A long-lived secret never flows into an instance more exposed
   than the secret itself.
7. **Egress is denied by default and allowed by declaration.** An instance
   may reach only the origins its definition implies — its own services and
   the third parties it names. The allowlist is derived, not hand-written,
   so it cannot drift from the stack it protects.
8. **Isolation is total by default.** Two instances share nothing
   stackless allocates — names, ports, data, credentials, network policy
   — unless the definition explicitly says so. Co-residency the
   substrate itself imposes (one host OS locally, one provider account
   in the cloud) is not shared state; the boundary phase narrows it.
9. **The cheapest substrate that can answer the question wins.** Local is the
   default; paid substrates exist for what localhost cannot prove.

## Non-goals

stackless is not a PaaS, not an infrastructure-as-code state manager, not a
CI system, not a production orchestrator, not a general-purpose firewall,
and not a framework. It does not deploy your production. It has no opinion
about how applications are built — only about how their disposable
incarnations live, prove themselves, stay contained, and die.

## v0 direction

Two substrates — the developer's machine and Render — chosen because
together they force every hard problem: local isolation and routing, remote
provisioning, paid resources, credential flow, verified remote teardown,
and — when its phase comes — the secret-blind egress boundary in both a
local and a cloud shape. v0 ships the lifecycle layer first; the boundary
is sequenced after it (ARCHITECTURE.md §0). One real product (atto: SPA +
Rust API + Postgres + third-party auth) is the dogfood from the first
commit: if stackless cannot express it, stackless is wrong. When the
boundary ships, "express it" includes routing the API's own calls to its
auth provider through the boundary so the secret never touches the
service.

## The test

A stranger — or an agent handed nothing but a repo with a stackless
definition — runs one command and gets a working, isolated, named copy of
the entire product with a URL they can open. They run one more command and
it's proven healthy. They walk away, and within the lease window it's
gone, verifiably, having cost cents. If any step needed a wiki page, a
teammate, or a manual cleanup, stackless has failed its purpose.

The boundary phase tightens the test: throughout, the agent never holds a
real durable secret, cannot reach anything the stack didn't declare, and
no secret is ever pasted by hand. v0 passes the lifecycle test on
test-scoped, operator-visible secrets (ARCHITECTURE.md §0); it does not
yet claim the tightened one.
