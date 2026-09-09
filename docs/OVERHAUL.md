# Lifecycle overhaul

Implementation ledger for the eight findings in the architecture review.
The eight architecture contracts are implemented for the advertised capabilities.
Stripe Projects remains the provisioning authority. The user approved the two
delta certifications in [DEPENDENCY-REVIEW.md](DEPENDENCY-REVIEW.md) on 2026-09-07.
Both trust entries are recorded. No dependency exemptions were added.
The complete `mise run ci` gate passed after recording the approved entries.

## Required contracts

- [x] **Ownership:** immutable instance identities; explicit owned, borrowed,
  and shared resources; no adoption by catalog type; teardown cannot remove a
  sibling's resources; legacy ownership conflicts fail closed.
- [x] **Recovery:** durable intent before every external creation; immediate
  resource registration; recovery after lost responses and process death;
  teardown includes unfinished operations and independently verifies absence.
- [x] **Controller:** one lifecycle owner per instance; CLI, SDKs, MCP, and
  reaper use the same operation service; durable operation IDs, cancellation,
  reconnectable progress; no shared-database multiwriter fleet execution.
- [x] **State:** desired revisions separate from observed existence,
  configuration, readiness, and unknown observations; truthful status and
  endpoints; retries reconcile changes instead of skipping existing objects.
- [x] **Stripe boundary:** explicit project/environment/resource context;
  isolated runtime directories; complete context-dependent operations are
  serialized; no runtime writes to application definitions; credentials are
  scoped handles, not ordinary operation results.
- [x] **Execution model:** workloads, jobs, resources, and endpoints;
  output dependencies separate from readiness dependencies; concurrent
  independent starts; source roots and immutable revisions; process and
  container execution; placement per resource, including mixed cloud targets.
- [x] **Provider contract:** common validation for check and execution;
  explicit capabilities; shared failure-injection and lifecycle tests;
  consistent verification, environment injection, revision deployment,
  observation, logs, and deletion across supported adapters.
- [x] **Authority boundary:** explicit trusted host execution; controlled child
  environments; privileged provider credentials stay in the controller;
  sensitive outputs are redacted from normal CLI/SDK/MCP results; sandboxed
  execution has an enforceable filesystem/process/network boundary rather
  than a naming convention.

## Implementation sequence

1. Correct ownership selection, observation parsing, and locking. Add tests
   that reproduce cross-instance reuse and concurrency failures.
2. Introduce resource inventory and durable operations. Migrate local and
   Render together, including interrupted creation and verified teardown.
3. Move execution into the controller. Migrate clients and lease enforcement.
4. Compile definitions into resource, workload, job, and endpoint plans.
   Centralize Stripe context, source resolution, and execution authority.
5. Migrate every supported provider and SDK. Retain explicit legacy teardown
   until old instances are gone. Update the schema, architecture, examples,
   generated contracts, and release documentation.

## Completion evidence

- Kill the controller before and after each external side effect. Restart it;
  verify no duplicate creation and no loss of teardown information.
- Create two instances with the same catalog services. Destroy either and
  observe the other independently. Repeat with reused display names.
- Exercise concurrent requests, process crashes, cancellation, lease expiry,
  provider timeouts, malformed inventory, revoked credentials, and failed
  deletion. Unknown observations must never become confirmed absence.
- Exercise updates, workers, migrations, verification jobs, dynamic endpoints,
  and mixed provider placement under the same engine contract.
- Test all public transports against the same controller and error semantics.
  Check that normal responses and child environments exclude privileged secrets.
- Run the README/mise gates and local end-to-end smokes. Record live provider
  evidence separately; hermetic tests cannot stand in for live capabilities.

## Final contract audit

| Review point | Implementation and evidence |
|---|---|
| 1. Ownership | Immutable birth IDs, explicit owned/borrowed/shared inventory, and receipt-based recovery. [Resource tests](../crates/stackless-core/tests/resources.rs) and [engine tests](../crates/stackless-core/tests/engine.rs) cover name reuse, conflicting handles, and deletion authority. |
| 2. Recovery | Intent precedes external creation; returned handles are saved before readiness. Unfinished records participate in teardown. The common engine recovery test reopens the store after creation succeeds and the step fails. Native lifecycle suites cover lost responses and separate native absence from catalog removal. |
| 3. Controller | [The controller](../crates/stackless/src/controller.rs) owns operations for CLI, SDK, MCP, reaper, and remote clients. [Operation tests](../crates/stackless-core/tests/operations.rs) and [controller tests](../crates/stackless/tests/controller_operations.rs) cover duplicate submissions, caller exit, SIGKILL recovery, cancellation, cursors, and remote uploads. Shared-database multiwriter execution is removed. |
| 4. State | [Desired/applied revisions](../crates/stackless-core/src/state/revision.rs) are separate from [current observations](../crates/stackless/src/client/report.rs). Status probes readiness and preserves unknown configuration on failed observations. Retained inventory includes removed workloads without exposing payloads. Native URL methods and health gates require recorded outputs. |
| 5. Stripe boundary | [Runtime context](../crates/stackless/src/client/runtime.rs) binds project/environment identity in private directories and holds the session lock through context-dependent work. Shared Stripe provisioning returns resource handles and scoped outputs. Tests cover sibling isolation, lost project/environment creation, and unchanged application definitions. |
| 6. Execution model | [The planner](../crates/stackless-core/src/engine/plan.rs) separates output, ready, and completed dependencies. [Routing tests](../crates/stackless-core/tests/routing.rs) cover concurrent placement and retained ownership. Jobs, workers, source roots, images, endpoints, TCP listeners, and mixed stacks have execution tests and examples in [EXECUTION.md](EXECUTION.md). |
| 7. Provider contract | [Capabilities](../crates/stackless-core/src/capabilities.rs) and provider validation run before admission. Every cloud adapter has a native lifecycle suite using the common engine and simulated APIs. Eight adapter tests now reject missing health URLs before polling. Shared client tests check each native URL decoder against changed deployment outputs. |
| 8. Authority boundary | [Controlled environments and redaction](../crates/stackless-core/src/security.rs) separate controller credentials from workload inputs and ordinary results. Host execution requires an explicit birth-scoped grant. [Container workloads](../crates/stackless-local/src/workload.rs) enforce filesystem, process, and network restrictions. Controller tests cover secret rotation, restart, grants, and container isolation. |

Capability limits are explicit. Native cloud finite jobs remain unsupported.
TCP is supported for local host processes; cloud and isolated-container TCP
admission is rejected. Host execution remains trusted execution. It does not
claim the container sandbox's authority boundary.

Native cloud tests use simulated APIs. No live cloud resources were created during
this overhaul. Live provider conformance must be recorded separately before claiming
live-tested behavior. The four gated workspace tests require public Git access or
a local Docker engine with cached images; earlier Docker validation is recorded
below. These limits do not change the mandatory ownership and teardown contract.

The two cargo-vet delta certifications are recorded with explicit user approval.
The complete `mise run ci` gate passed on 2026-09-07 in 124.30 seconds: formatting,
lint, catalog checks, 935 tests passed with four skipped, and audit/deny/vet.
Cargo-vet reports two partially audited dependencies and 372 exempted dependencies;
the existing baseline exemptions remain. No new exemptions were added.
See [the approved reviews](DEPENDENCY-REVIEW.md). Earlier approval blockers in the
progress history below describe the state before this authorization.

Final validation: `mise run check` passed. The full workspace suite passed
**935 tests**, with four gated tests skipped across 46 binaries. An earlier run
flagged a capture leak in a pure HTML-selection test; its isolated rerun and the
final full run passed without that flag. The TypeScript build and 22 SDK tests,
20 Python tests, and Go tests passed in the preceding placement phase; their
implementation did not change in this final audit. The TCP CLI lifecycle passed
check, doctor, up, verify, status, logs, down, and final status in isolated state.
The repository agent skill passed its validator. `cargo audit` and `cargo deny`
passed; `cargo vet` reports only the two unapplied delta certifications.

## Progress history

These entries record implementation stages. The final audit above supersedes
earlier lists of remaining work. Initial foundations:

- OS file locks release on process death and serialize daemon startup. Database
  operation claims use a unique token, reject overlapping calls in one process,
  and cannot be stolen from another machine based on elapsed time.
- Stripe deployable lookup no longer adopts a sibling by catalog type. Failed
  or malformed inventory cannot establish resource absence.
- Each instance birth gets an immutable ID. Resource records belong to that ID
  and survive name reuse. Legacy provider lookup names remain unchanged.
- The resource inventory records intent, creation, and confirmed absence.
  Borrowed and shared references never grant deletion authority. Teardown reads
  unfinished records as well as completed checkpoints.
- Local service shells wait on a pipe until their PID and start time are saved.
  Caller death before release closes the pipe without running user code. Render
  saves intent before provisioning and handles before configuration or deploy.
- Tombstoning, revival, and garbage collection reject unresolved teardown
  evidence. Instance mutations and verification run under an operation claim.
- `check --on` calls the provider validator. Local empty commands fail before
  execution. The database and daemon socket use owner-only permissions.

- Provider calls receive display name, immutable ID, resource namespace, and
  checkpoint handles separately. New cloud names are bounded to 52 bytes and
  local source/log paths use the immutable namespace. Legacy names are retained.
- Stripe commands run in private directories keyed by instance ID. A session
  lock covers context selection through credential use. Application definitions
  are never edited by lifecycle setup. Shared project anchors live in the
  controller database; project creation has durable intent and exact-name
  recovery. An unknown create result cannot trigger a second create request.
- Stripe environments have owned inventory entries and verified deletion.
  Instance context outlives step resources. Failed children block deletion of
  their parents. Vault reads do not inherit another environment's combined file.
- Cloud log readers use supplied checkpoint handles rather than opening the
  default database. Garbage collection runs as a queued operation tied to the
  expired birth and removes its private runtime and logs.

Catalog integrations now persist creation intent, submission state, and returned
handles before environment attachment and configuration. Hosting adapters still
need complete native resource intent and recovery. Legacy cloud ownership
needs an explicit migration audit.
Mixed placement is implemented below. Complete hosting recovery and the final
authority audit remain required.

- CLI, Rust, TypeScript, Python, Go, MCP, and the reaper use one controller.
  Requests, operation IDs, results, cancellation, and progress are persisted.
  Each instance runs one operation at a time; different instances can overlap.
- A lost submission response retries the same ID. Restart resumes unfinished
  up/down operations and marks interrupted verification for explicit retry.
- Shared remote-database lifecycle execution is rejected. SSH controller
  transport is implemented; Linux deployment verification remains required.
- TypeScript subprocess calls no longer block the event loop. Real-controller
  submission, failure propagation, status, and history passed for all three
  language SDKs. Controller SIGKILL recovery passed after local service start.
  This does not establish recovery for every provider or hook kill point.

Validation includes store reopen after failed creation, reusable-name history,
borrowed/shared deletion exclusion, legacy migration, lock takeover exclusion,
SIGKILL before process release, and state-file permissions. Hermetic workspace
checks are recorded during each implementation stage; live provider verification
remains required before claiming provider lifecycle completion.

- Lifecycle results now expose typed secret references scoped to immutable
  instance IDs. All four SDKs use the reference type; language SDK parsers reject
  plaintext and foreign references. Service interpolation remains private.
- The controller persists redaction values before workload injection and when
  resource outputs are recorded. Results, errors, and returned logs are redacted.
  Values survive rotation and restart; unprotected legacy results are withheld.
- Workload, prepare, and verify children inherit a controlled environment.
  Infrastructure credential names and known-value aliases cannot be injected.
  Logs are created with mode 0600 in mode 0700 instance directories.
- A real daemon test passed app-secret injection, controller environment
  exclusion, error and log redaction, credential rotation, and daemon restart.
- Local Docker workloads now use copied sources, a non-root user, a read-only
  root filesystem, dropped capabilities, process and memory limits, and an
  isolated network. A pinned ingress proxy exposes HTTP through loopback.
  A real Docker test verified those boundaries, controller restart, and teardown.
  See [execution contracts](EXECUTION.md). Cloud sandbox parity and common
  provider conformance remain unfinished.

- Container jobs recover the same running container after controller death.
  Setup and prepare hooks are not repeated within that operation. An image-only
  job retains its default command and filesystem. The real Docker tests pass.
- Teardown uses each resource generation's recorded parent keys. Reversing a
  workload dependency across revisions no longer creates a false teardown cycle.
  A failed deletion protects its prerequisites while independent generations
  can be removed.
- Stripe catalog creation records submission before `projects add`, then saves
  the response before environment attachment. An unknown create result triggers
  discovery. An empty inventory cannot authorize a second create. Shared plans
  have instance-level references and never grant deletion authority.
- Environment creation follows the same submission rule. Failed responses are
  recovered by discovery; teardown keeps unresolved submitted intents.
- Definition snapshots, desired revisions, and the current definition change
  in one transaction. Both database backends pass rollback tests. Comment edits
  can retain the same semantic revision.
- Catalog resources retain the remote provisioning ID and catalog identity.
  Creation checks remote inventory before submission. Recovery checks all pages
  and rejects ambiguous names. Teardown journals removal before sending it and
  independently reads the exact remote ID. Only `removed` proves deletion;
  pending, failed, malformed, and unavailable responses remain unresolved.
- The remote transport uses the Stripe CLI login and Projects 0.35's internal
  provisioning API contract. A read-only live probe confirmed project and
  resource pagination envelopes and the full resource status response, including
  `removed`. This is not a live create/delete conformance run. See
  [Stripe recovery](STRIPE-RECOVERY.md) for the compatibility boundary.
- Structured Stripe failures never trigger a plaintext retry. That prevents
  repeated creation and interactive shared-plan cleanup after a failed request.

- SSH connects clients and MCP to the same remote owner with strict host-key
  verification. Source uploads are durable before extraction. A real stdio bridge
  test passed upload, deletion of caller files, controller restart, idempotent
  replay, and teardown. TypeScript, Python, and Go pass controller-selection tests.
- A systemd user unit and deployment instructions are included. Controller info
  checks the live unit PID, enablement, restart policy, and user lingering before
  reporting persistence. No running Linux host has verified deployment or reboot.
- Provider context checks now reject missing or mismatched Stripe projects and
  missing environments. Adapters cannot initialize, relink, or write project
  anchors to application definitions. The controller owns those setup mutations.

- Vercel projects and shared plans now use the catalog journal. Deployment POSTs
  carry a persisted receipt in provider metadata. Recovery scans every page and
  refuses a second POST when a submitted receipt remains unresolved. Exact IDs,
  project IDs, and receipt metadata are checked before native deployment deletion.
- A Vercel engine test passed failed POST, store reopen, resume, and teardown
  against mocked Stripe and Vercel backends. It created one project and one
  deployment, then deleted the deployment before the catalog project. Separate
  tests passed lost delete responses, malformed pagination, and foreign receipts.
  These are hermetic tests; Vercel live conformance remains unverified.
- Vercel readiness checks the recorded deployment. Environment writes use upsert,
  credential refresh errors cannot switch to another team's token, and upload
  collection excludes credentials and rejects symlinks through SourceArchive.

- Cloudflare uploads persist intent and a desired revision before PUT. Worker
  tags bind the native script to its immutable instance owner. Recovery reads
  settings and revision annotations before retrying. An unresolved upload or a
  foreign owner blocks mutation. Deletion requires an independent absence read.
- Workers account enablement has shared references. Instance teardown removes
  only its owned scripts. A two-instance engine test verified that deleting one
  leaves the other's script present and retains account registrations. Separate
  tests passed lost upload/delete responses and refused malformed settings.
  These are mocked API tests; live Cloudflare conformance remains unverified.
- Cloudflare no longer advertises runtime logs. The old implementation returned
  deployment metadata and suppressed API failures. Actual runtime log retrieval
  remains required before enabling that capability.

- Render service creation now uses the catalog journal. Deployment attempts save
  a pinned Git commit, complete pre-submit inventory, and submission state before
  POST. HTTP 201 preserves the returned deployment ID. Lost or queued responses
  recover only one new API deployment of that commit; ambiguous results block
  another POST. Auto-deploy is disabled before taking the inventory snapshot.
- Render readiness polls the recorded deployment ID and uses the returned service
  endpoint. A failed or superseded deployment cannot succeed by following a newer
  deployment. Service lookup rejects ambiguous names and malformed inventory.
- Render native deletion is persisted before DELETE. Native absence and catalog
  removal are verified separately. An engine test passed lost deployment and
  deletion responses, store reopen, resume, and cleanup with one service creation,
  one deployment POST, and one DELETE. Live Render conformance remains unverified.
  Cloud hook process recovery and sandbox execution remain unfinished.

- Netlify catalog sites retain native site creation, build/deployment receipts,
  and deletion submission. Static, ZIP, and Git deployment POSTs persist intent
  before submission. Lost responses recover the receipt; queued builds retain
  their build ID. Unknown or ambiguous inventory cannot trigger another POST.
- Netlify native deletion precedes catalog removal. Exact native identity and
  independent absence checks protect unfinished resources. Runtime logs are
  marked unsupported instead of returning deployment metadata as log output.
- Cloudflare, Vercel, and Netlify source collection now walks configured roots
  through directory descriptors without following symlinks. Parent paths and
  absolute paths cannot escape the checkout. Archives exclude the provider token
  files used by this repository. Common source pinning remains unfinished.
- Netlify tests passed lost Git/ZIP build responses, queued build-ID recovery,
  native site-creation recovery, changed-source rejection, and malformed or
  ambiguous inventory. The engine test passed static upload and deletion
  recovery with one POST and one DELETE. Workspace validation after these changes:
  749 tests passed, eight skipped; Clippy and Rust/TOML formatting passed.
  These checks use local mocks and do not establish live provider conformance.

- Render, Vercel, Netlify, and Cloudflare now save operation-owned source archives.
  Submission of local source writes follows durable intent. The sealed manifest
  can recover before database registration. Branch movement and upstream deletion
  cannot change a resumed operation's source. Old snapshots remain owned until
  teardown; a sibling's ownership ID cannot authorize their deletion.
- Upload collection reads the sealed archive. Prepare and verification use a
  separate working copy of the same snapshot. Hook edits cannot change upload
  bytes. Render deploys the saved commit; Vercel sends it in `gitSource.sha`.
  Deployment revisions omit source storage paths and operation IDs, so identical
  source content does not force another deployment merely because its copy moved.
- Netlify Git builds still use a branch. Five other hosting adapters still need
  source migration. Cloud hook execution recovery, sandbox parity, snapshot
  retention limits, and process-kill conformance remain required.
- Git caches now retain the remote's advertised default branch. Resolving `HEAD`
  no longer substitutes the cache's synthetic `main` branch when the upstream
  default differs. A fixture verifies both the initial default and a later change.
- Source snapshot validation passed interruption before database registration,
  branch movement, upstream deletion, hook/upload isolation, and sibling-safe
  teardown. Workspace validation: 754 passed, eight skipped; Clippy and Rust/TOML
  formatting passed. These tests do not establish live provider or process-kill
  conformance for the new source helper.

- Common `source.root` and provider `root` now resolve through one validator.
  Conflicting directories, absolute paths, parent traversal, and protected paths
  fail before provisioning. The four snapshot adapters validate the selected
  directory during materialization and use it for hooks and upload collection.
- Vercel file uploads clear `rootDirectory` after cropping the archive. Git
  deployments retain the selected root. Both send explicit build settings so
  omitted settings cannot inherit an earlier deployment's configuration. Render
  sends the root in web-service catalog configuration, updates the native service,
  and independently reads it back before the pinned deployment. Observation
  reports a changed root as drift. Tests reject a foreign service before PATCH
  and an update that the following GET does not confirm.
- Root validation and provider regression checks passed: 760 workspace tests,
  eight skipped; Clippy, Rust/TOML formatting, catalog ownership, and provisional
  provider checks passed. Netlify saved build-setting resets, provider config-file
  overrides, and Render's initial catalog-created static build remain required.
- The supply-chain gate was run against current advisories. Vet passed with 431
  exemptions. Audit and deny failed on
  [RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258.html):
  `h2 0.3.27` through libsql's Hyper 0.14 dependency, and `h2 0.4.14` through the
  newer HTTP stack. Audit also reported the existing `anyhow 1.0.102` unsoundness
  advisory. Dependency remediation remains required; no new advisory exceptions
  were added.

- Removed the unused libsql driver and its old HTTP/TLS dependencies. The store
  now uses SQLite under one controller. Direct remote open and obsolete fleet
  configuration return `state.remote.disabled` without connecting or silently
  creating a local database. Remote clients use the existing controller transport.
- Legacy fleet SQL exports convert their schema marker and apply later migrations
  in one transaction. Tests retain checkpoint payloads, leases, instance IDs,
  unfinished resources, and foreign-host claims across reopen. Incomplete and
  newer exports fail without rewriting their schema. This preserves teardown
  evidence; it does not replace the outstanding legacy cloud ownership audit.
- CI and nightly fleet jobs now run controller regression tests. They do not
  claim live SSH login or systemd reboot verification. Fleet documentation describes
  the controller transport and the private export migration procedure.
- Updated `h2` to 0.4.16 and the optional WebAssembly tooling's `anyhow` lock entry
  to 1.0.103. Removed the five old advisory exceptions. Audit reports zero
  vulnerabilities and no warnings; deny passes advisories, bans, licenses, and
  sources. Cargo-vet still needs the two [proposed delta audits](DEPENDENCY-REVIEW.md).
  Automatic approval review rejected recording those persistent trust entries.
  No audit entries or new exemptions were applied.
- Workspace regression after this change: 764 passed, four skipped, 42 binaries.
  The four removed ignored tests exercised the retired direct Turso backend.
  Its state semantics now run in the legacy-export fixture tests. Clippy and Rust
  formatting passed; both edited CI workflows parse as YAML. No live provider
  or controller host migration was performed.

- WordPress and GitLab now materialize durable source snapshots before prepare
  and deployment. Both resolve common and provider root aliases through the
  shared validator. Prepare uses the snapshot working copy; publication reads
  the sealed archive. Neither deployment path clones the upstream repository.
- WordPress selects `index.html`, then the first HTML path in lexical order.
  Missing HTML and invalid UTF-8 fail before site provisioning. GitLab commits
  the saved UTF-8 files to the target project's default branch, independent of
  the upstream ref. Binary GitLab assets remain unsupported by its commit encoder.
- Adapter tests reopen SQLite after deleting the upstream repository and working
  copy, then publish the original bytes after prepare edits its copy. Both tests
  check sibling deletion rejection and owned snapshot cleanup. Existing legacy
  source-ref and catalog teardown tests remain. These mocks do not establish
  native creation/deployment recovery or live provider conformance.
- Workspace validation: 769 passed, four skipped, 42 binaries. Clippy and Rust/TOML
  formatting passed. Railway, Fly, and Laravel Cloud still need snapshot migration.
  Five adapters, including WordPress and GitLab, still need native deployment
  journals and independent deletion evidence. Cargo-vet's two proposed delta
  audit entries remain pending explicit approval and have not been applied.


- GitLab no longer changes a private repository to public during Pages deployment.
  Repository visibility and Pages access are separate settings in the
  [GitLab access-control contract](https://docs.gitlab.com/user/project/pages/pages_access_control/).
  Existing catalog visibility configuration remains the provisioning input.
- GitLab retains the full SHA returned by commit creation and selects pipelines
  with that SHA and the target branch. Each status read checks project ID,
  pipeline ID, branch, and SHA. Missing pipelines wait within the deployment
  budget. Failed, canceled, skipped, ambiguous, malformed, or foreign pipelines
  cannot satisfy readiness. The Pages job must also report success.
- GitLab now requires an active root deployment in the
  [Pages settings response](https://docs.gitlab.com/api/pages/). Empty inventory
  waits; malformed or ambiguous inventory fails. A guessed project URL cannot
  substitute for a deployment. The Pages response has no commit identity, so
  matching the serving bytes to the submitted commit remains unfinished.
- Commit actions now use the
  [base64 encoding](https://docs.gitlab.com/api/commits/#create-a-commit-with-multiple-files-and-actions)
  supported by GitLab. Binary assets survive upload. Source paths retain their
  layout beneath the generated `public/` directory. Unicode API error truncation
  no longer slices inside a character.
- Regression evidence: 777 workspace tests passed, four skipped, 42 binaries.
  Clippy passed. New tests cover private repository preservation, wrong commit
  identity, ambiguous pipelines, failed pipelines, delayed pipeline creation,
  missing Pages deployments, malformed active inventory, and binary uploads.
  Native commit-submission recovery, current serving-revision proof, and native
  deletion evidence remain required; successful pipeline history is insufficient.


- GitLab now journals catalog creation and native commit submission. Native
  project IDs persist before API configuration or deployment. Commit receipts,
  returned SHAs, pipeline IDs, and job IDs survive restart. Missing or ambiguous
  receipt inventory cannot authorize another commit submission.
- A reserved `.well-known/stackless-deployment.json` file identifies the serving
  deployment. Readiness and observation read it with an unauthenticated client
  and a 4096-byte limit. A different receipt reports revision drift. Source cannot
  supply this reserved path. Reconciliation after external revision drift and
  removal of stale files from earlier uploads still require work.
- GitLab teardown unpublishes Pages before deleting the project, then waits for
  native absence before catalog removal. The
  [GitLab.com retention period](https://docs.gitlab.com/api/projects/#delete-a-project)
  can leave a deleted project pending for 30 days. The inventory retains it.
  Legacy catalog-only records keep their existing teardown path pending audit.
- The engine fixture loses catalog creation, commit submission, Pages removal,
  project removal, and catalog removal responses. Recovery sends each mutation
  once, detects a different serving receipt, and rejects a sibling's teardown.
  A separate fixture keeps a soft-deleted project owned across retries. These
  are API mocks and database-reopen tests, not process-kill or live conformance.
- Workspace validation: 783 tests passed, four skipped, across 42 binaries.
  Clippy and Rust formatting passed. WordPress, Railway, Fly, and Laravel Cloud
  still need native deployment journals and independent deletion evidence.
  Cargo-vet's two proposed delta audit entries remain unapplied pending approval.


- Fly source builds now use the shared durable snapshot for materialization and
  prepare, then extract the sealed archive for the remote builder. Start no
  longer reclones the branch. Common and provider root aliases use the shared
  validator; Dockerfile paths are relative to the selected build context.
  Image-only services without prepare do not fetch an unused source repository.
- The builder clears inherited environment variables, supplies its app-scoped
  deploy token, and uses a disposable home outside the build context. Generated
  configuration is outside the context and passed explicitly. Failure output
  redacts deploy tokens and application environment values; Unicode truncation
  respects character boundaries. This is credential confinement, not a sandbox
  or durable builder supervision.
- The Fly adapter fixture reopens SQLite, removes the upstream and working copy,
  runs prepare, and invokes a fake builder through the real Start path. It checks
  original bytes, nested Dockerfile handling, omitted credentials and Git data,
  and sibling source deletion rejection. Separate tests cover invalid roots,
  escaped Dockerfiles, unused image sources, and redacted builder errors.
  Native Fly mutation journals, builder process recovery, and live conformance
  remain required. Railway and Laravel Cloud remain on the old source path.
- Validation: 787 workspace tests passed, four skipped, across 42 binaries.
  Final Fly tests passed all 30 cases. Workspace Clippy and Rust/TOML formatting
  passed. The pending cargo-vet audit entries remain unapplied.


- Laravel Cloud now rejects a common source repository that differs from its
  catalog repository. Source root aliases use the shared validator. Before
  deployment, native application ID, name, repository, and root must match.
  Environment selection follows the application's explicit default relationship;
  environment ownership and branch are checked instead of choosing the first
  production-like name from inventory.
- Deployment reads check ID, environment, branch, full commit hash, and an exact
  status vocabulary. Unknown states remain API faults. A successful deployment
  must also be the running or hibernating environment's current deployment.
  URLs come from recorded vanity domains or the exact primary-domain relationship.
  Missing, malformed, foreign, or ambiguous inventory cannot supply guessed URLs.
- Native API clients reject invalid credentials before sending a request, disable
  redirects, and preserve response-read failures. Error truncation handles Unicode.
  Start checkpoints now include the actual commit and branch, with defaults for
  old checkpoints. These are readiness checks, not durable submission recovery.
- Primary API evidence: Laravel's
  [initiate endpoint](https://laravel.com/cloud/docs/api/deployments/initiate-deployment)
  and official CLI request model have no commit selector. The
  [deploy hook contract](https://laravel.com/cloud/docs/deployments#deploy-hooks)
  supports `commit_hash`; the adapter still needs that transport and lost-response
  recovery. `root_directory` is creation-only in the native API and absent from
  the Stripe catalog schema. Snapshot migration, root creation, source-provider
  identity, native mutation journals, and live conformance remain unfinished.
- Validation: 795 workspace tests passed, four skipped, across 42 binaries.
  Laravel's 27 tests include wrong application and repository relationships,
  replaced commits, malformed inventory, historical deployments, and stopped
  environments. Workspace Clippy and Rust formatting passed. Cargo-vet's two
  pending delta audit entries remain unapplied.

- Laravel Cloud now journals catalog creation, the native application ID,
  deployment submission, returned deployment ID, and first observed commit and
  branch. Database reopen resumes a known deployment without another POST.
  An unreturned deployment ID stays unknown and blocks resubmission, including
  from a later revision. No deployment is adopted from a timestamp or inventory
  difference because the native initiation API has no request receipt.
- Native Laravel observation checks the recorded deployment against the current
  environment pointer. Teardown saves deletion intent, verifies native absence,
  and only then removes Stripe registration. Lost catalog creation can be torn
  down without a start checkpoint. Unknown deletion, revoked credentials,
  malformed responses, and foreign application identity retain ownership.
- New tests run the real engine with simulated catalog and native failures,
  reopen SQLite, reject sibling teardown, and check one create, deployment POST,
  native DELETE, and catalog removal per request. A separate poll test rejects
  a commit replacement after reopening the database. These are hermetic tests,
  not process-kill or live provider evidence. Laravel immutable source transport,
  creation-time roots, provider-host identity, and unknown-POST recovery remain
  unfinished. WordPress, Railway, and Fly still need native mutation journals.
- Validation: 800 workspace tests passed, four skipped, across 42 binaries.
  Laravel Cloud's 32 tests passed. Workspace Clippy, Rust formatting, and diff
  checks passed. No dependency versions changed in this phase; the two pending
  cargo-vet delta audit entries remain unapplied.

- WordPress now shares a catalog/native ownership journal. Numeric site IDs
  and origins persist before publication. Page creation has revision receipts,
  submission intent, exact recovery by slug and metadata, and immediate page-ID
  registration. Unknown or conflicting evidence never triggers another create.
- WordPress readiness checks page content, publication status, homepage
  configuration, and an anonymous public serving receipt. Content and homepage
  drift can be repaired on the same recorded page, with durable intent and
  readback. Lost applied-update responses do not repeat the update on restart.
- WordPress teardown keeps native identity after Stripe subscription removal,
  then deletes any remaining native site and verifies 404. A lost cancellation
  response or unconfirmed native deletion retains ownership. Engine tests cover
  lost catalog creation, page creation, homepage update, catalog cancellation,
  and native deletion responses, database reopen, and sibling exclusion.
  Missing receipts, changed site IDs, malformed responses, and revoked tokens
  cannot authorize creation or teardown. Public receipt requests exclude OAuth
  headers and reject responses above 2 MiB.
- These remain hermetic tests, not process-kill or live WordPress evidence.
  Superseded pages, legacy ownership, and platform-specific HTML preservation
  still need conformance work. Native journals remain to be added for Railway
  and Fly. WordPress's new serving and ownership checks do not complete the
  eight-point overhaul.
- Validation: 807 workspace tests passed, four skipped, across 42 binaries.
  WordPress's 31 tests and Laravel Cloud's 32 tests passed. Workspace Clippy,
  Rust formatting, and diff checks passed. The negative-response fixtures use
  bounded global mocks so every injected response is verified. No dependency
  versions changed; the two pending cargo-vet delta audit entries remain unapplied.

Fly now journals native app identity, IP allocation, and machine deployment
submission beside the catalog receipt. Image and source retries recover one
exact machine receipt after lost responses. Changed image revisions update the
recorded machine with its observed version; source revisions restrict flyctl to
the recorded machine. Readiness checks configuration, image digest, started state,
and the complete app machine inventory. Source builds apply guest settings and
keep application secrets out of command-line arguments.

Fly teardown records deletion before the native request, verifies app absence,
and persists that proof before catalog removal can revoke the deploy token.
Unknown, malformed, and foreign identity responses retain ownership. Tests cover
engine recovery after lost catalog, machine, and removal responses; native
absence before catalog removal; sibling exclusion; missing and ambiguous
receipts; IP-allocation recovery; changed revisions; and source-build recovery
without invoking the builder again. These are simulated failures with store
reopen, not real controller kills or live Fly evidence. Remote-builder resource
ownership and process supervision, same-revision drift repair, legacy ownership,
and live provider conformance remain required.

Validation for this Fly phase: all 816 workspace tests passed, with four gated
skips. After the final command-argument ordering correction, all 39 Fly tests
passed again. Workspace Clippy and formatting passed. No dependency or TOML
changes were made in this phase. The two pending vet trust certifications remain
unapproved; supply-chain files were not changed.

Railway source refs now resolve through the durable snapshot helper. Prepare runs
from the saved working copy at `source.root`; deployment sends the recorded SHA
with the documented `commitSha` argument. An image without prepare avoids an
unused Git checkout. Service-instance updates now place service/environment IDs
in the GraphQL arguments rather than the settings input. Source, root, start
command, and replacement variables are read back before deployment. New
checkpoints verify native deployment identity, commit metadata, and the sole
active deployment pointer. Malformed envelopes and unknown status values fail.

Railway still needs native creation journals and independently verified teardown.
Creating a Git-backed service can trigger a deployment before root settings and
the explicit commit request. That auto-deploy race, persistent Git triggers,
config-file overrides, and live provider conformance remain required. The tests
prove snapshot recovery and request/readback behavior against simulated APIs;
they do not establish native crash recovery or live provider behavior.

Validation for the Railway source phase: all 821 workspace tests passed, with
four gated skips. After the final endpoint correction, all 27 Railway tests
passed again. Workspace Clippy, Rust formatting, and all 60 TOML formatting
checks passed. The only new dependency edge is the existing workspace Git test
helper. The two pending vet trust certifications remain unapproved; supply-chain
files were not changed.

Railway now persists native project and service receipts, mutation fingerprints,
submission state, and returned IDs beside catalog ownership. Project recovery
reads every inventory page and checks receipt, name, and workspace. Service
recovery checks the owned parent and reserved receipt variable. Empty service
creation precedes source/root configuration. Applied settings and variable
writes recover through readback. Domain recovery requires one domain under the
recorded service and environment. Known deployments resume by ID; a lost deploy
response without an ID remains unknown and blocks resubmission.

Railway observation now checks configuration fingerprints and domain identity in
addition to deployment identity, commit, and active pointer. Teardown saves its
intent, requires the matching native project deletion record, and persists that
proof before catalog removal. Missing inventory and GraphQL authorization errors
retain ownership. Engine tests lose catalog, project, service, settings,
variables, domain, poll, and removal responses, reopen SQLite, and verify that
mutations are not repeated. Separate tests reject malformed or ambiguous
inventories, foreign receipts, changed workspaces, and changed configuration.
These are simulated APIs and database reopen tests, not controller kills or live
Railway evidence. Tombstone visibility, linked-account scope, implicit source
connection deployments, Git triggers, config-file overrides, image digest proof,
same-revision drift repair, and legacy ownership audit remain required.

Validation for the Railway journal phase: all 829 workspace tests passed, with
four gated skips, across 42 binaries. All 35 Railway tests passed. Workspace
Clippy, Rust formatting, and all 60 TOML formatting checks passed. The new direct
Chrono dependency uses the already locked 0.4.45 version to parse deletion
timestamps. No dependency version changed. The two pending vet trust
certifications remain unapproved; supply-chain files were not changed.

GitLab Pages now reconciles its managed repository files. It reads the tree at a
full branch SHA, deletes stale `public/` assets, updates the desired files with
per-file commit checks, and preserves unrelated repository paths. The commit
plan's base SHA and action digest persist before submission. The returned
commit's file set is read back. Malformed trees, duplicate paths, foreign file
metadata, and unverified branch absence stop the mutation.

Completed GitLab deployments now get a new repair generation when the branch tip
or public serving receipt changes. The old request remains recorded. A lost
repair response recovers the new receipt after SQLite reopen without another
commit. Tests verify stale asset deletion, binary preservation, file/directory
replacements, unchanged unrelated files, serving-only drift, and recovery after
losing the repair response. API clients reject invalid credentials before HTTP,
disable redirects, and propagate response-read failures. These remain simulated
provider tests. File-level checks are not a transaction with other Git writers;
failed-CI retries, legacy requests without completion evidence, private Pages
access, controller-kill coverage, and live conformance remain required.

Validation for this GitLab phase: all 836 workspace tests passed, with four gated
skips, across 42 binaries. All 43 GitLab tests passed. Workspace Clippy, Rust
formatting, and diff checks passed. No dependencies or TOML files changed in this
phase. The two pending vet trust certifications remain unapplied.

Controller input collection now removes retained remote source bytes. Migration
013 adds request digests and an input-retirement marker. A terminal operation's
private body expires after seven days once its immutable instance birth is gone
and its resources are absent. Unbound failures wait until their alias has no
instance; queued or running work prevents collection. Existing request bodies
supply digests when retired. IDs, results, events, and retry identity survive.

Collection derives full and partial upload paths from the accepted request,
removes those files, then retires the database input. Missing paths make retries
safe after a lost database update. Symlinked roots or targets retain the request
for inspection. Collection runs on startup and the operator's reaper tick.
Admitted upload directories already had resource records and teardown cleanup;
this phase adds cleanup for failures before admission and private operation
bodies. SQLite can reuse freed pages. Database shrinking and secure erasure of
backups are outside this mechanism.

Tests cover retained active inputs, reused aliases, pending work, legacy digest
backfill, changed-payload rejection, partial directories, sibling preservation,
and symlink refusal. A real controller restart test verifies that a retired
remote request still returns its original success through the stdio bridge.
The failed-admission fixture reopens SQLite and invokes controller collection;
it does not inject a process kill during directory deletion.

Validation for input collection: all 841 workspace tests passed, with four gated
skips, across 42 binaries. Workspace Clippy, Rust formatting, and diff checks
passed. No dependencies or TOML files changed in this phase. The two pending vet
trust certifications remain unapplied. The eight-contract overhaul remains open.

Fly source builds now use a journaled local runner. Build intent precedes
workspace creation. PID, process start time, invocation cookie, and native
submission intent commit before a pipe gate releases flyctl. A retry reconnects
to that process before accepting its native machine receipt. No recorded command
is launched twice. Preparation without a committed process stamp can resume; unfinished legacy builder
submissions without process receipts fail closed.

The runner drains combined output while retaining its first 64 KiB. An independent
watchdog stops its process group at the workload deadline, default 300 seconds.
Cancellation and teardown also find detached helpers by exact invocation cookie.
Teardown stops the local builder and removes its private workspace before native
API reads or deletion. Revoked credentials retain native app ownership without
leaving the local builder running. Build directories are excluded from source
archives. Nonzero exits, cancellation, and deadlines use `fly.build.stopped`.
Process liveness now excludes exited children that have not yet been reaped.

Tests kill the runner's parent and verify its deadline, prevent execution before
gate release, bound a 200000-byte output stream, stop a helper that creates a
separate session, and preserve an unrelated process with a cookie substring. Fly fixtures drop the execution future, reopen
SQLite, remove the builder executable, and recover the same process and machine.
They also cover cancellation, deadline expiry, no repeated launch after failure,
sibling rejection, and builder cleanup before a native authorization failure.
These use a fake flyctl and simulated native API. The watchdog covers its process
group; detached helpers require the controller's cookie scan. Fly's remote
builder resources, provider-side cancellation, failed-build retry generations,
full-controller kill coverage against Fly, and live provider conformance remain
required.

Validation for builder supervision: all 850 workspace tests passed, with four
gated skips, across 42 binaries. All 43 Fly tests passed in the combined targeted
run. The final five process tests passed after extending detached-helper coverage.
Workspace Clippy, Rust formatting, and diff checks passed. A concurrent isolated
Fly run hit startup timeouts; subsequent separate targeted and workspace runs
passed. No dependencies or TOML files changed. The two pending vet trust
certifications remain unapplied. The eight-contract overhaul remains open.

### Cloud prepare command ownership

All nine cloud adapters run prepare through the shared durable command runner.
A stored host-execution grant is checked before provider setup and again in the
runner. Each accepted operation owns a `cloud-command` resource. Its key binds
instance, operation, step, and provider. Its input digest binds the command,
effective environment, source commit and archive digest, working directory, and
timeout. Changed inputs cannot replace a submitted command.

The process waits behind a gate until its identity is committed. Launch and
teardown use a process lock, with a fresh inventory read after acquiring it.
Recovery waits for the saved process. An exit receipt of zero and a stopped
process are required before a completed checkpoint can be reused. Cancellation,
deadline expiry, and teardown stop the recorded process and helpers carrying its
invocation cookie. The command depends on its source and prerequisite resource
keys, so teardown removes it before those parents. Empty lock files remain.
Output retention is capped at 64 KiB; the rest is drained. Command workspaces
are excluded from source archives. The standalone convenience helper also has
bounded output and a 300-second deadline, but has no durable inventory.

Laravel Cloud now materializes the shared Git snapshot and runs prepare from its
working copy. Its native deployment API still follows the connected branch.
This change does not prove immutable Laravel deployment transport.

Five new tests cover host-grant rejection, interruption and SQLite reopen with
one hook execution, changed-input rejection, cancellation, deadline expiry,
sibling ownership rejection, teardown, bounded output, secret redaction, launch
locking, and missing successful receipts. The existing Laravel engine fixtures
now execute a real prepare hook and assert one execution across provider failure
and controller-store recovery. The archive test includes a private command-output
canary. These tests drop prepare futures; they do not kill the full controller
while a cloud prepare hook runs. The core runner's separate process-death tests
cover its watchdog. Helpers that remove the cookie, cloud setup, native cloud
jobs, full-controller cloud-hook kill coverage, and live provider conformance
remain required.

Validation: 855 workspace tests passed, with four gated skips, across 42 binaries.
The 54-test cloud/Laravel run passed with the engine retry coverage. The final
archive exclusion test passed after adding its command-output canary. Workspace
Clippy, Rust formatting, all 60 TOML formatting checks, and diff checks passed. An earlier
workspace run failed controller-start fixtures while a second test run was
active; the separate workspace rerun passed. This does not establish the cause
of the startup failures. Only the existing workspace `stackless-git` dependency
was added to Laravel's test dependencies. The two pending vet trust
certifications remain unapplied. The eight-contract overhaul remains open.

### Cloud setup execution

All nine cloud adapters now execute declared setup hooks through the owned
command runner. They no longer record setup as a successful action without
running it. Setup and prepare have distinct receipts, share the same saved
working copy, and use the same host grant, environment filtering, process gate,
deadline, cancellation, and teardown rules. Setup failures use
`execution.setup_failed` with the command and service in the error context.
Prepare keeps each adapter's existing error code.

Hook revisions now include the source snapshot key. Content digests alone were
wrong for setup: a new working directory at the same commit could reuse an old
setup checkpoint and leave dependencies uninstalled. Setup revisions also include
secret names and the timeout. Each new cloud operation initializes its own copy;
recovery of the same operation reconnects to the saved execution. Fly and Railway
image services skip Git only when neither setup nor prepare needs it. Empty
sources remain unsupported by the cloud capability contract.

Two new tests cover interrupted setup recovery before prepare consumes its
output, and failed setup retaining one execution receipt with the setup error
code. The existing Fly and Railway image tests now exercise setup, materialized
source, and grant rejection. Laravel engine tests model a persisted operation ID
across database reopen, then a second accepted operation at the same commit.
Both working copies contain one setup execution and one prepare execution.
An initial fixture incorrectly expected two snapshots from four unregistered
engine invocations. It now submits, starts, recovers, and finishes explicit
operation IDs. This is operation-store simulation, not a full controller kill.

Native cloud jobs, source-free cloud execution, enforceable sandbox boundaries,
full-controller cloud-hook kill coverage, and live provider conformance remain
required. The eight-contract overhaul remains open.

Validation for setup: all 857 workspace tests passed, with four gated skips,
across 42 binaries. Nextest marked the controller SIGKILL recovery test as leaky;
its separate rerun passed without a leak report. The cause remains unproven.
All 134 targeted cloud, Laravel, Fly, and Railway tests passed. Workspace Clippy,
Rust formatting, and diff checks passed. No TOML or external dependency changes
were needed. The two pending vet trust certifications remain unapplied.

### Local finite-command deadlines and output

Local shell jobs, setup, and prepare now use the shared durable command runner.
The resource key binds instance, operation, and step; the input digest binds
command, directory, environment, and revision. Input changes cannot launch a
replacement within the same operation. Intent is saved before workspace
creation. The PID, start time, invocation cookie, and deadline are saved before
the process gate opens. Launch and teardown share a process lock and reread the
inventory after acquiring it. Teardown checks the instance owner, inventory key,
paths, and workspace marker before stopping processes and removing files.
Submitted resources with an unlaunched payload fail validation.

The watchdog enforces the command budget while the controller is dead. Exit
receipts include the cause, so an explicit exit 124 remains `job.failed` and a
watchdog expiry becomes `job.timeout`. Older numeric receipts remain readable.
Older local job records reconnect to their original receipt without launching
another process. They retain the old process-group teardown and have no cookie
or independent watchdog.

Each execution retains at most 64 KiB of combined output. The rest is drained.
The shared runner now writes retained bytes directly: its previous buffered
reader lost short output when the watchdog killed it before EOF. Local `logs`
reads the newest owned execution for each finite step, including failed commands
without successful checkpoints. The existing client redactor applies saved
secrets. The substrate log interface receives the caller's store so inventory
lookup uses the same database. Unknown workload names fail validation. Reads of
long-running service logs are also capped at 64 KiB, but those files still grow
while the process runs.

Five new tests cover explicit-exit versus timeout receipts, retained bounded
failure output, same-operation input changes, sibling cleanup rejection,
malformed submitted inventory, legacy receipt recovery, and a full controller
SIGKILL during a timed job. The controller stays dead until the watchdog stops
the command. Restart recovers the same failed operation, retains one launch,
blocks its dependent, redacts its output, and removes its workspace on down.
The output assertion exposed the buffered-reader bug. The workspace run also
exposed a PID-file publication race in the detached-helper fixture; that fixture
now closes the temporary file before renaming it into place.

Native cloud jobs, verification execution, source-free cloud execution,
enforceable sandbox boundaries, detached helpers that remove their cookie,
legacy escaped-process recovery, long-running host-service log retention,
full-controller cloud-hook kill coverage, and live provider conformance remain
required. The eight-contract overhaul remains open.

Validation for local finite commands: all 862 workspace tests passed, with four
gated skips, across 42 binaries. The 11 controller tests also passed in the prior
targeted run, with two gated skips. Workspace Clippy, Rust formatting, and diff
checks passed. No TOML or external dependency changes were needed. The initial
workspace run stopped at the detached-helper fixture race described above; the
corrected full run passed. The two pending vet trust certifications remain
unapplied.

### Verification execution journal

Verification now records a controller-owned `verification-command` before
releasing its process gate. The key binds instance, operation, and tier; the
input digest binds command, directory, resolved environment, and timeout.
Recovery reconnects to the saved PID, start time, and invocation cookie. Changed
inputs cannot launch another command in the same operation. The resource depends
on the existing inventory, so teardown stops verification before removing its
source or prerequisites. The managed substrate recognizes this resource across
providers and validates its owner, key, paths, and workspace marker on cleanup.

`[stack.verify]` and named tiers accept `timeout_secs`, default 300, range 1
through 86400. The independent watchdog enforces the deadline while the
controller is dead. Timeout returns `verify.timeout`; explicit exit 124 remains
`verify.failed`. Each command retains the first 64 KiB of combined output, drains
the rest, and keeps failed output available through `logs`. Saved secrets are
redacted on read. The Rust client exposes `submit_verify` to return an operation
ID before waiting.

The controller publishes a journal-version event before verification execution.
Restart queues these operations with their original IDs. A persisted cancellation
request remains runnable until a worker stops the recorded processes and saves
the cancelled result. Older verification operations without the event remain
interrupted. The event marks the execution contract, not command success.

Cloud verification reuses initialized working copies. If a copy is missing,
restoration initializes it through a separate setup receipt. Recovery waits for
that setup before the proof starts. Losing a copy already initialized by this
verification operation produces a source error instead of repeating setup.
Legacy Render and Vercel source references with no usable checkout now materialize
an owned source snapshot and journal setup. The unused unbounded local
`Spawner::run_hook` helper was removed.

Six new tests cover bounded failure output, same-operation input rejection,
SQLite reopen, host-grant rejection, sibling and marker cleanup rejection,
default and tier budget validation, journal-version recovery, pending cancellation,
legacy source materialization, restored-copy initialization, and full controller
SIGKILL during successful and timed verification. The cancellation crash fixture
stops the controller before persisting cancellation, kills it, then verifies that
restart stops the still-running command before reporting cancellation. It retains
failed redacted output and removes all command workspaces on down.

The first controller run exposed the old state rules that interrupted every
verification and rejected running cancellation. Those rules now distinguish the
journaled contract. A later fixture compared macOS `/var` and `/private/var`
spellings of the same output path; it now compares canonical paths.

Provider verification conformance, native cloud jobs, source-free cloud execution,
enforceable sandbox boundaries, long-running host-service log retention, legacy
escaped-process recovery, and live provider evidence remain required. Shared
Git materialization still has no operation-driven cancellation of an in-flight
fetch. Detached helpers that remove their invocation cookie remain outside the
host runner's process ownership proof. The eight-contract overhaul remains open.

Validation for verification: all 868 workspace tests passed, with four gated
skips, across 42 binaries. All 13 controller tests passed in the targeted run,
with two gated skips. The final cancellation-crash fixture passed separately
after tightening cleanup on assertion failure. Workspace Clippy, Rust formatting,
all 60 TOML formatting checks, and diff checks passed. The existing workspace
`stackless-git` dependency enables its fixture feature for this crate's tests;
no external dependencies were added. The two pending vet trust certifications
remain unapplied.

### Bounded helper capture and process identity

The shared helper runner caps stdout at 4 MiB and stderr at 256 KiB. Crossing
either limit stops the invocation and returns a capture failure, including when
the child exits zero. `Finished` requires complete EOF on both streams. The
reader owns one capped buffer; it no longer grows and clones an unlimited vector.
Nonblocking descriptors let the controller stop and join readers after a shared
two-second drain deadline. An open writer cannot leave a blocked reader thread
behind or turn partial output into a successful response.

Descendant observations now retain PID and start time. Cleanup checks that
identity before signaling, and cookie lookup matches the exact
`STACKLESS_SPAWN=<invocation>` environment entry. A substring in another variable
or cookie does not authorize termination. Cleanup rescans matching helpers for
up to three seconds and reports failure if any remain. The durable workload
runner also passes its saved process stamp into tree cleanup instead of reducing
the identity to a bare PID.

Stripe maps capture limits, incomplete output, and failed process cleanup to
separate fault codes. Errors omit captured bytes and configuration arguments.
The JSON parser only receives complete bounded output. The daemon's persistence
probes also reject these runner failures. Seven new tests cover exact byte limits,
overflow after exit zero, stopping an output flood, joining a reader with an open
writer, exact cookie matching, stale process identities, safe error reporting,
and an oversized response beginning with valid empty-inventory JSON. That response
must remain an error rather than confirmed resource absence.

These helper deadlines still run inside the controller. Independent enforcement
after controller death, helpers that remove their cookie and escape observation,
enforceable sandbox boundaries, long-running host-service log retention, native
cloud jobs, provider conformance, and live provider evidence remain required.
The eight-contract overhaul remains open.

Validation for helper capture: all 875 workspace tests passed, with four gated
skips, across 42 binaries. All 25 targeted process and Stripe tests passed.
Workspace Clippy, Rust formatting, and diff checks passed. Catalog ownership
checks covered all 92 deployables, and the three provisional references matched
their existing allowlist. No dependencies or TOML files changed. The prior
60-file TOML check remains applicable. The two pending vet trust certifications
remain unapplied.

## Host-service log retention

New host services run under a separate internal CLI process. Its command stays
behind a pipe gate until the resource inventory contains the runner's PID,
start time, and invocation cookie. The runner owns the service process and its
collector. Killing the controller no longer removes log collection.

The collector writes directly in chunks of at most 8 KiB and rotates exactly at
1 MiB. It retains three generations, at most 3 MiB per service. Short messages
are visible while the service is running. The log reader takes the newest
64 KiB across generations, including messages split by rotation. File identity
checks avoid reading the same inode twice during concurrent renames. Existing
large generations are capped before a replacement launches. Container log
snapshots remove obsolete host generations before replacing the current file.

A writer lock excludes concurrent collectors for the same service. Namespace
directories and log destinations reject symlinks and nonordinary file types.
Write or rotation failure kills the service generation. SIGTERM leaves the
collector alive for shutdown output. Teardown allows five seconds for the
service to exit and the runner allows at most two seconds to drain its pipes.
Remaining helpers are killed by recorded process identity and exact invocation
cookie. A dead runner with surviving helpers produces runtime drift, so retry
replaces it. Teardown checks fresh ownership records before touching processes.
Submitted records with an unlaunched payload fail closed. Resolved service env
values enter the redaction history before launch.

The runner ships in the existing CLI binary. An embedded Rust controller needs
that CLI through PATH or `STACKLESS_BIN`. `TestContext::with_cli` supplies a
binary explicitly and runs a private controller for hermetic SDK tests. Tokio's
signal feature keeps the collector alive during group termination. It uses the
already-locked signal dependencies and adds no dependency version.

Seven tests cover rotation and immediate short writes, legacy oversized files
and symlink rejection, the reexecuted native runner, collector write failure,
shutdown output, inventory ownership and phase checks, and controller death.
The controller test writes 8 MiB while the controller is dead, checks all three
file caps, restarts and reads redacted output, kills only the runner, observes
drift, then proves `down` removes the surviving service process.

This supersedes the earlier unbounded-log limitation for newly launched host
services. Legacy running services retain their direct file descriptors until
replaced and their writes remain unbounded. Old receipts have no invocation
cookie. Retained logs remain until namespace garbage collection. This is not a
filesystem or process sandbox against host code executing as the operator.

The eight-contract overhaul remains incomplete. Remaining work includes native
cloud jobs, source-free cloud workloads, complete mixed endpoint support,
provider conformance and live evidence, enforceable sandbox boundaries, legacy
escaped-process recovery, and independent deadlines for generic provider CLI
helpers. Two supply-chain trust certifications remain unapplied.

Validation:

- Full workspace: **882 passed, 4 gated tests skipped**, across 42 test binaries.
- Controller, SDK, and process/log subset: **25 passed** before the final
  inventory guard test. The full workspace run includes that guard test.
- Clippy passed for the workspace with all features and targets, warnings denied.
- Rust formatting, TOML formatting across 60 files, and `git diff --check` passed.
- Initial compilation caught the drift payload type and two renamed interfaces.
  Those were fixed before the passing full run.

## Independent provider command deadlines

Stripe CLI execution now runs through `HelperCommand` and the internal
`stackless daemon helper` process. The helper owns the 90-second command budget,
output capture, and cleanup. Its caller can die without removing the deadline.
Every launchctl invocation, plus systemctl and loginctl persistence checks, uses
the same runner with a five-second command budget. Real-user ID lookup reads the
OS directly instead of spawning an unbounded `id -u` command.

The command description stores raw executable and argument bytes, the working
directory, and an explicit environment. Inheritance is captured at construction;
clearing the environment, removing keys, and overriding values survive the helper
boundary. CLI resolution moved into core and remains reexported by the daemon.
Internal helper entrypoints bypass self-update and ordinary command dispatch.
An embedded caller can supply the CLI executable explicitly.

The helper takes the Stripe command lock before acknowledging launch. The caller
then releases a second gate. Closing that gate cannot execute the command.
After release, the helper keeps the lock through execution, cleanup, and output
capture. A competing invocation cannot interleave while the original caller is
dead. Runtime snapshot replacement waits for the same lock. Garbage collection
leaves the instance and its private files intact while the lock is held.

The decoded output caps remain 4 MiB for stdout and 256 KiB for stderr. The
versioned pipe envelope is capped at 6 MiB and preserves binary output and exit
status. A missing, malformed, truncated, or oversized result cannot become
successful command output. Capture-limit and cleanup faults retain the existing
Stripe fault codes and omit partial output and configuration arguments.

The first full run exposed a cancellation bug in shared process cleanup. Killing
an unordered set could kill `sleep` before its shell, allowing the shell to run
the next command. Cleanup now builds the owned parent order from stamped process
identities, suspends parents before children, then kills the collected processes.
Durable command cleanup passes the entire invocation to this operation instead
of killing one cookie member at a time. The regression test supplies the prior
bad iteration order and checks that the waiting parent cannot write its marker.

Added tests cover environment clearing, raw arguments, full output limits, invalid
results, lock contention, both launch gates, caller SIGKILL, detached children,
parent-first cleanup, snapshot replacement, and deferred garbage collection. A
real CLI integration test invokes the actual Stripe driver against a local fake
executable, kills its caller, proves a second command cannot acquire the context
lock, then lets the original command finish and verifies lock release.

These deadlines do not undo provider requests already accepted remotely. Resource
journals still govern lost-response recovery. Independently killing the helper,
or escaping both recorded ancestry and the invocation cookie, remains outside
this proof. Native cloud jobs, source-free cloud workloads, complete mixed
endpoint support, provider conformance and live evidence, enforceable sandbox
boundaries, and legacy escaped-process recovery remain required. The full eight
contracts remain open. Two supply-chain trust certifications remain unapplied.

Validation:

- Full workspace: **896 passed, 4 gated tests skipped**, across 43 test binaries.
- Focused process, helper, and cloud-hook suite: **37 passed** after the cleanup fix.
- Real CLI/helper subset: **11 passed** before the final cleanup and GC additions.
- Clippy passed with all workspace features and targets, warnings denied.
- Rust formatting and `git diff --check` passed. No TOML or dependency versions
  changed in this phase; the preceding 60-file TOML check remains current.
- An early fixture used a filename macOS rejects. It now uses a Unicode directory
  and separately verifies raw non-UTF-8 argument transport. The cancellation
  failure was a production bug and was fixed before the passing full run.


## Source-free cloud images

Fly and Railway now accept common `image` fields without dummy Git remotes.
Both advertise image and empty-source support. Provider image aliases still
work; conflicting aliases, source-free builds, nonexistent source roots, and
ambiguous command overrides fail validation before admission. Common `run`
uses a shell inside the image. Fly records `init.exec`; Railway records a
quoted `startCommand`, including separate quoting for legacy command arrays.

Cloud snapshots now distinguish Git from an empty source. Existing receipts
without a kind decode as Git. Empty snapshots record a digest and no commit;
readback still checks the sealed archive. Setup and prepare use the owned
working copy and their existing execution receipts. A missing copy restores
from the empty archive. Materialization revisions include the hook and image
configuration, so adding hooks invalidates an earlier skipped-source step.

Verification can create a journaled empty workspace when an image deployment
has an action checkpoint instead of a checkout. Repeated calls reuse the same
workspace. A missing copy already initialized by that verification operation
fails instead of silently repeating setup. The legacy Git verification path
shares this materializer and retains its pinned commit.

Hermetic coverage includes source-free Fly and Railway deployment requests,
lost responses, database reopen, native observation, and ordered teardown.
Additional cases cover empty archive recovery between sealing and registration,
archive corruption, hook execution and teardown, image alias conflicts,
argument boundaries, and verification workspace recovery. These tests use
simulated provider APIs. No live provider evidence was added.

Native cloud workers and finite jobs, source-free workloads on other cloud
adapters, and the remaining eight-contract acceptance work are still open.

Validation for this change: the full workspace suite passed 903 tests with four
skipped. Nextest marked one existing process-identity test leaky; all assertions
passed. A focused rerun passed that test without a leak flag. After the final
Fly `PORT` correction, all 45 Fly tests and that process test passed together.
Workspace clippy passed with warnings denied; the final Fly change also passed
its all-target clippy gate. Initial failures were two missing test struct fields,
a test borrow conflict, sandbox socket denial, and teardown tests calling the
legacy checkpoint API instead of the resource-inventory API. Those were fixed
or rerun with the required local process access. No live provider APIs were used.
The final `mise run check` gate passed: formatting, workspace clippy, Taplo,
catalog ownership, and provisional integration validation. Full supply-chain
certification and live provider conformance remain separate outstanding work.


## Native Fly workers

Fly now advertises workers for both images and source builds. Without HTTP
health, requests omit listeners and public-IP allocation; namespaces and results
omit origins. Worker restart policy is `always` in the Machines API and generated
fly.toml. Workers with explicit HTTP health retain that listener. Origin
references, root origins, and endpoint declarations without HTTP health fail
validation before admission. The source-build receipt includes worker mode while
preserving the old fingerprint format for HTTP source deployments.

Readiness re-observes the owned machine after start. It requires the saved
receipt, configuration, image digest, exclusive app inventory, and started state.
A stopped or changed machine fails with `fly.worker.not_ready`. Status no longer
reports non-HTTP workloads with configuration drift as ready. Fly worker logs
use the existing `fly_events` source; runtime stdout/stderr remains unavailable.

Each Fly up reconciles its start step. A stopped or suspended worker records a
start request keyed by operation before its Machines API POST. Machine ID,
version, and deployment receipt are saved with that request. Recovery confirms
started state through readback and completes the request. A start with unknown
outcome cannot authorize another POST, even from a new operation. A new desired
deployment cannot bypass that unresolved start. Teardown retains the owned app
and machine throughout this path.

New tests cover image worker recovery and native restart after lost responses,
source-build worker recovery without HTTP configuration, origin validation,
unknown starts across database reopen, native event logs, and controller status
when a worker runner disappears. They exercise mock provider APIs and real local
processes. Native finite jobs and live Fly conformance remain required; the
full eight-contract objective is still open.

Validation: 908 workspace tests passed, four skipped, with no leak flags. The
standard `mise run check` gate passed. The final status-stage change then passed
all 15 enabled controller integration tests, workspace clippy with warnings
denied, and formatting. Initial focused failures came from an overly strict
mock IP-query count, the wrong test constructor, and killing a test runner
before its application startup barrier. The corrected fixtures passed. No live
Fly resources were created.

### Named endpoint URLs

Endpoint declarations now supply `${endpoints.<name>.url}` to workload, hook,
and verification environments. Provider namespaces bind aliases after resolving
native origins. Explicit URLs retain their own names and do not replace service
origins. Validation requires an HTTP workload, an HTTP or HTTPS URL with a host,
and no embedded credentials. Integration configuration rejects endpoint refs.

The planner distinguishes declared URLs from provider start outputs. Declared
URLs do not erase service-origin or explicit readiness dependencies. Late-bound
self references fail as cycles. Consumer revisions include only referenced
endpoint declarations and resolved URLs, not entire target checkpoints.

CLI, Rust, TypeScript, Python, Go, and controller results expose endpoint maps.
Status carries URL source and readiness; declared URLs remain unverified.
Generated IDL and language bindings retain endpoint names and workload targets.
Local containers bind dynamic aliases through their isolated network and reject
external or host-process endpoint references.

Public URL interpolation no longer enters redaction history merely because a
command receives it as an environment variable. Sensitive names, literal values,
composite expressions, and independently registered secrets still redact.
Retained redaction history is never removed to expose an endpoint.

Tests cover planning, alias resolution, unknown outputs, changed URLs, selective
reconciliation, container admission, Render deployment URL changes, generated
bindings in all four languages, and a real local controller lifecycle including
CLI output, verification, resume, health failure, and teardown. Custom-domain
provisioning, non-HTTP endpoints, cloud finite jobs, and mixed placement remain
unimplemented. This does not complete the eight required contracts.

Validation: `mise run check` passed. `mise run test` passed 918 tests with four
skipped. TypeScript build and 17 tests, Python 15 tests, and Go SDK tests passed;
these execute the generated endpoint bindings as well. The generated TypeScript
fixture also passed strict type checking. No live cloud resources were created.


### Mixed provider placement

One routing adapter now composes all desired and retained hosting providers.
Workloads and catalog resources use their own `on` placement, with the instance
substrate as the default. The engine schedules one dependency graph and records
placement before external operations. Migration 14 recovers old hosting identity
from checkpoints and unfinished resource inventory. Removed definitions retain
routing records. Reassignment is rejected while a checkpoint or unfinished
resource exists, and becomes available after verified teardown.

Every namespace builder accepts the router's combined origins. Late cloud URLs
come from deployment receipts; six remaining guessed-URL interpolation paths
were removed. Local and Fly advertise early origins. Target placement controls
URL dependencies, and both native-origin and named-endpoint URL changes enter
consumer revisions. Local container admission still rejects endpoints outside
its isolated network.

All client paths construct the same router: admission, execution, teardown,
route recovery, status, logs, check, and verification. Local-default instances
with cloud workloads prepare the mandatory Stripe context. Verification source
resolution uses the anchor workload's selected provider. Source pins, including
inherited pins, are checked against the selected adapter. Up and check results
expose workload/resource placement maps; status exposes `on`. Rust, TypeScript,
Python, and Go expose the new results.

Core tests exercise concurrent starts across two adapters, cross-provider URLs,
resume, rejected live reassignment, retirement after reopening state, reassignment
after verified absence, and teardown after a lost create response. A native Fly
adapter with mocked HTTP/Stripe APIs runs alongside a real local finite job.
That test covers lost catalog creation, URL injection, completed-job reuse,
dependency-ordered teardown, and lost catalog removal. No live cloud resources
were created. TCP endpoints, a native cloud finite-job path, and the full
eight-contract completion audit remain required.

Validation: `mise run check` passed. `mise run test` passed 930 tests with four
skipped across 46 binaries. The TypeScript build and 22 tests, Python 20 tests,
and Go SDK tests passed. The built CLI accepted `fixtures/mixed/stackless.toml`
and returned `api: fly` and `check: local` in its placement map.

### TCP listeners

`health = { protocol = "tcp" }` models local host listeners. The controller
records the allocated port before releasing user code. Native origins and named
endpoints use `tcp://127.0.0.1:<port>` from that receipt. TCP creates no HTTP
proxy route. Output references wait for start; readiness dependencies wait for
a connection. A workload cannot consume its own dynamically allocated URL at
start. HTTP-only fields and root-origin routing are rejected for TCP.

TCP status distinguishes successful connections, connection refusal, and unknown
network observations. Declared URLs remain unverified. Cloud adapters and local
isolated containers reject TCP admission until they implement tested routing.
Workers without listeners no longer receive invented local origins; references
to those origins fail validation.

The controller test runs a real TCP listener and dependent job, verifies URL
injection, reuses completed work on resume, kills and restarts the controller,
replaces the listener, observes refusal while its process remains alive, and
tears it down. Schema tests cover protocol matching, port requirements, dependency
edges, and unsupported provider combinations.

Validation: `mise run check` passed. The workspace suite passed **933 tests** with
four gated tests skipped across 46 binaries. The built CLI passed check, doctor,
up, verify, status, logs, down, and final status for `fixtures/tcp` in a temporary
state directory. Its process and embedded daemon were stopped afterward.

The final audit follows the original eight review points. That review requires
jobs in the execution model and explicit provider capabilities. Native finite
jobs on every cloud adapter are not an acceptance condition; Fly jobs remain
unsupported. Earlier progress entries overstated that requirement. Ownership,
cancellation, Stripe context, and status inventory are still under final review.
