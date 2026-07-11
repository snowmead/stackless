# AGENTS.md

Repository overview, project docs, and provider tooling live in
[CLAUDE.md](CLAUDE.md), [README.md](README.md), and [ARCHITECTURE.md](ARCHITECTURE.md).
Standard dev commands are documented in the README "Development" section and in
`mise.toml` `[tasks]` — use those as the source of truth.

## Cursor Cloud specific instructions

The startup update script runs `mise install`, so the pinned Rust 1.96.0
toolchain plus `cargo-nextest`, `taplo`, `cargo-audit/deny/vet/dist`, and `prek`
are already present, and the `prek` git hooks are already wired.

- **Activate the mise tools before running tasks.** New shells need
  `eval "$(/home/ubuntu/.local/bin/mise activate bash)"` on `PATH` (already
  appended to `~/.bashrc` for interactive shells) so `cargo-nextest`/`taplo`
  resolve. Alternatively prefix commands with `mise exec --`.
- **Standard gates** (defined in `mise.toml`): `mise run check` (fmt + clippy +
  taplo), `mise run test` (`cargo nextest run --workspace --all-features`),
  `mise run ci` (adds the supply-chain audit/deny/vet). Plain `cargo build` /
  `cargo test` also work.
- **Running the product end-to-end (local substrate).** Build once
  (`cargo build`) then drive `./target/debug/stackless`. The CLI auto-spawns its
  own daemon (reverse proxy on `:4444`, supervision, lease reaper) on demand — no
  manual daemon start. A working local smoke without a remote repo:
  `stackless up --name demo --on local --file fixtures/hello/stackless.toml --source web=<dir-with-index.html-containing "hello-fixture">`,
  then `curl http://demo.localhost:4444/`, then `stackless down demo`.
- **`--source svc=PATH` pins a service to a local checkout**, bypassing git
  materialization — the way to exercise `up` locally without a reachable
  `source.repo` remote (the committed fixtures point at `example.invalid`).
- **Expected DEGRADED persistence warning.** `status`/`list` print
  `⚠ DEGRADED: leases enforced only while the daemon happens to be running ...`
  because the VM has no launchd/systemd init for boot persistence. This is
  non-blocking — leases and every verb still work while the daemon is alive.
- **Live cloud smokes** (`mise run smoke-*`, `discover`, `stripe-refresh`) need
  real provider credentials (`STRIPE_API_KEY`, `VERCEL_TOKEN`, `RENDER_API_KEY`,
  usually via `.stackless.env`) and are excluded from the hermetic gates; they
  cannot run here without those secrets.
- **Cloud lease reaping runs from the operator's daemon.** A sleeping operator
  machine defers cloud lease expiry until wake; spend caps bound leakage.
