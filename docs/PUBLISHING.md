# Publishing stackless packages

Release surfaces, lockstep with `[workspace.package].version` (today
`0.1.71`):

| Surface | Package | How it ships |
| --- | --- | --- |
| crates.io | `stackless` + workspace crates | tag `vX.Y.Z` → `publish-packages.yml` |
| npm | `stackless-sdk` | same tag |
| PyPI | `stackless-sdk` (`import stackless`) | same tag |
| Go module | `github.com/snowmead/stackless/sdks/go` | tag `sdks/go/vX.Y.Z` |
| GitHub Release / installer | CLI binaries | same `vX.Y.Z` via cargo-dist `release.yml` |

Language SDK versions live in
[`sdks/typescript/package.json`](../sdks/typescript/package.json) and
[`sdks/python/pyproject.toml`](../sdks/python/pyproject.toml). They must match
the workspace version (`scripts/check-sdk-versions.sh`).

`stackless-provider-sdk` is a catalog-provider surface. It is not required to
consume the environment SDK, but it is published in the crates wave below.

---

## One-time registry setup

1. **crates.io** — Create an API token with publish scope. Store it as the
   GitHub Actions secret `CARGO_REGISTRY_TOKEN`.
2. **npm** — Own the unscoped package name `stackless-sdk` (the npm
   user/org `@stackless` is already taken by a third party). Create a
   granular access token with publish rights. Store as secret `NPM_TOKEN`.
3. **PyPI** — Create project `stackless-sdk` (the name `stackless` is taken).
   Prefer [Trusted Publishing](https://docs.pypi.org/trusted-publishers/):
   - GitHub environment name: `pypi` (matches `publish-packages.yml`)
   - Workflow: `publish-packages.yml`
   - Repository: `snowmead/stackless`
4. **Go** — No registry token. Consumers resolve via `proxy.golang.org` after
   the `sdks/go/vX.Y.Z` tag is pushed.

---

## Release checklist

1. Bump `[workspace.package].version` and matching
   `[workspace.dependencies]` version pins in the root `Cargo.toml`.
2. Set the same version in `sdks/typescript/package.json` and
   `sdks/python/pyproject.toml`.
3. Update `CHANGELOG.md` (cargo-dist reads the matching section for Release
   notes).
4. Open a PR; wait for CI including the `sdk` job (`mise run sdk-test` locally).
5. Merge, then from the release commit:
   ```bash
   git tag v0.1.8
   git tag sdks/go/v0.1.8
   git push origin v0.1.8 sdks/go/v0.1.8
   ```
6. Confirm:
   - GitHub Release + installer (cargo-dist)
   - `publish-packages` workflow green (crates.io, npm, PyPI)
   - `go list -m github.com/snowmead/stackless/sdks/go@v0.1.8` resolves

Manual re-publish of an existing tag (e.g. language SDKs after crates.io
already shipped):

```bash
gh workflow run publish-packages.yml --ref v0.1.71 -f tag=v0.1.71
```

Use `--ref vX.Y.Z` so the tag-gated GitHub environment `pypi` allows the job.

---

## crates.io order

Publish leaf → root. Skip `xtask` (`publish = false`). The automation is
[`scripts/publish-crates.sh`](../scripts/publish-crates.sh); it skips crates
already uploaded at the current version.

1. `stackless-core`
2. `stackless-git`
3. `stackless-stripe-projects`
4. `stackless-provider-sdk`
5. `stackless-cloud`
6. `stackless-daemon`
7. `render-client` / `vercel-client`
8. `stackless-local` and each hosting substrate crate
   (`stackless-render`, `stackless-vercel`, `stackless-fly`,
   `stackless-netlify`, `stackless-railway`, `stackless-laravel-cloud`,
   `stackless-gitlab`, `stackless-wordpress`, `stackless-cloudflare`)
9. `stackless-integrations`
10. `stackless-idl`
11. `stackless-bindgen`
12. `stackless`

Manual fallback:

```bash
mise exec -- cargo publish -p stackless-core
# …repeat in the order above…
mise exec -- cargo publish -p stackless
```

A failing mid-wave publish usually means an unpublished workspace dependency.
Publish that dependency first, then retry.

Later waves only need a version bump + tag.

---

## npm / PyPI / Go manual fallback

```bash
# npm
(cd sdks/typescript && npm ci && npm publish)

# PyPI
(cd sdks/python && python -m pip install build && python -m build \
  && python -m twine upload dist/*)

# Go (tag only)
git tag sdks/go/v0.1.71
git push origin sdks/go/v0.1.71
```

---

## Versioning

Workspace `version` is the single source of truth
(`[workspace.package].version`). Bump it once per release wave, keep
`workspace.dependencies` version pins and language SDK versions in sync, then
tag.
