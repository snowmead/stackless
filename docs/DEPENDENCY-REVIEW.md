# Approved dependency delta audits

The user explicitly approved these two `safe-to-deploy` delta certifications on
2026-09-07. They are recorded in [supply-chain/audits.toml](../supply-chain/audits.toml)
with automated differential review attribution. The earlier automatic approval
rejection is resolved by that authorization.

Validation: `mise run ci` passed after certification. Cargo-vet reports two partially
audited dependencies and 372 exempted dependencies. Existing exemptions are unchanged.

The review covers registry source differences for two version updates. Existing
baseline exemptions remain. This is not a full audit of either package.

## h2 0.4.14 to 0.4.16

The update fixes unbounded empty DATA frames described in
[RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258.html).
The source review covered DATA-frame accounting, header limits, padding
arithmetic, stream reset and wakeup handling, write-zero handling, mutex release
during IO, and HPACK buffer and decoder changes. The runtime manifest only raises
the existing `http` dependency's minimum version from 1 to 1.1.

All 3840 generated Huffman decode entries were checked against the unchanged
257-symbol encoding table, including branch destinations, consumed-bit counts,
symbols, and EOS rejection. The review did not add a dependency exemption or
run the upstream package's separate test suite.

Recorded entry for `supply-chain/audits.toml`:

```toml
[[audits.h2]]
who = "Codex (automated differential review)"
criteria = "safe-to-deploy"
delta = "0.4.14 -> 0.4.16"
notes = "Reviewed the registry source and manifest delta: bounded DATA-frame accounting, header limits, padding arithmetic, stream reset and wakeup handling, write-zero handling, lock release during IO, and HPACK buffer and decoder changes. Verified all 3840 generated Huffman decode entries against the unchanged 257-symbol encoding table. No new runtime dependencies or external IO authority. Differential review only; the baseline remains exempted."
```

## anyhow 1.0.102 to 1.0.103

The update fixes the context-downcast aliasing defect described in
[RUSTSEC-2026-0190](https://rustsec.org/advisories/RUSTSEC-2026-0190.html).
The changed downcasts derive raw field addresses without first creating a shared
reference. The TypeId guards and allocation ownership remain unchanged. Added
upstream tests mutate each context level and verify destruction. The build script
and runtime dependencies are unchanged.

The package remains in Cargo.lock through optional WebAssembly tooling. It is not
a direct dependency of a Stackless crate. Recorded entry:

```toml
[[audits.anyhow]]
who = "Codex (automated differential review)"
criteria = "safe-to-deploy"
delta = "1.0.102 -> 1.0.103"
notes = "Reviewed the registry source and manifest delta. Context downcasts derive raw field addresses without creating an intermediate shared reference, fixing RUSTSEC-2026-0190. TypeId guards and allocation ownership are unchanged; added tests mutate every context level and verify drops. Build script and runtime dependencies are unchanged. Differential review only; the baseline remains exempted."
```
