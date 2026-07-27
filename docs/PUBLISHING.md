# Publishing stackless crates

The product crate is **`stackless`** (lib + bin). Workspace members it
depends on must be on crates.io first (or in the same publish wave) because
`[workspace.dependencies]` pins both `path` and `version`.

`stackless-provider-sdk` is a different surface (catalog providers). It is
not required to consume the environment SDK.

## Order

Publish leaf → root. Skip `xtask` (`publish = false`).

1. `stackless-core`
2. `stackless-git`
3. `stackless-stripe-projects`
4. `stackless-provider-sdk`
5. `stackless-cloud`
6. `stackless-daemon`
7. `render-client` / `vercel-client` (if not already published)
8. `stackless-local` and each hosting substrate crate
   (`stackless-render`, `stackless-vercel`, `stackless-fly`, …)
9. `stackless-integrations`
10. `stackless-idl`
11. `stackless-bindgen`
12. `stackless`

## Dry-run

From the workspace root (requires network to talk to crates.io for
yank/existence checks; does not upload):

```bash
mise exec -- cargo publish -p stackless-core --dry-run --allow-dirty
# …repeat in the order above…
mise exec -- cargo publish -p stackless --dry-run --allow-dirty
```

A failing dry-run for a mid-wave crate usually means an unpublished
workspace dependency. Publish that dependency first, then retry.

Verified locally (package + compile, upload aborted):

- `cargo publish -p stackless-core --dry-run --allow-dirty` succeeds
- `cargo publish -p stackless-git --dry-run --allow-dirty` succeeds
- `cargo publish -p stackless --dry-run` fails until the substrate /
  integrations wave is on crates.io (expected before the first release)

## Versioning

Workspace `version` is the single source of truth
(`[workspace.package].version`). Bump it once per release wave, keep
`workspace.dependencies` version pins in sync, then publish.
