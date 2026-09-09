# Workloads and execution

A workload is an HTTP or TCP service, a worker, or a finite job. `services` and
`workloads` name the same table. `jobs` sets `kind = "job"` automatically.
Services require `health`. Workers can omit it. Jobs finish with an exit
status and cannot declare health.

```toml
[stack]
name = "example"

[jobs.migrate]
image = "my-registry.example/app@sha256:..."
run = "./migrate"
timeout_secs = 300

[workloads.web]
image = "my-registry.example/app@sha256:..."
run = "./server"
health = { path = "/health", status = 200 }
depends_on = { migrate = "completed" }
```

Fly and Railway accept HTTP workloads with `image` and no `source`. Their
provider blocks are optional when the common `image` field is present.
Provider-specific image aliases remain accepted; conflicting values fail
validation. A source-free workload cannot select a source subdirectory.
Other cloud adapters still reject this capability before admission.

For these image deployments, `run` replaces the image startup command through
`/bin/sh -c`. The image must contain `/bin/sh`. Fly sends `config.init.exec`;
Railway sends a quoted `startCommand`. An omitted `run` retains image defaults.
Fly sets `PORT` to its internal health-listener port unless the workload defines it.
`fly.cmd` changes Docker CMD while retaining ENTRYPOINT. `railway.cmd` supplies
exec-form start arguments, with each argument quoted separately. `run` cannot
be combined with either command array and is currently rejected for source
builds. See the [Fly init contract](https://fly.io/docs/machines/api/machines-resource/)
and [Railway start command contract](https://docs.railway.com/deployments/start-command).

Image workloads without setup or prepare skip source materialization. With
hooks, an omitted source creates an owned, sealed empty snapshot and a private
working copy. Its receipt records `kind = "empty"`, a digest, and no Git commit.
The hook output stays in that working copy. Recovery reuses the receipt and
teardown removes the workspace after its commands. Verification of an image
workload without a checkout creates its own recorded empty workspace.

Fly also accepts `kind = "worker"` for images and source builds:

```toml
[workloads.consume]
kind = "worker"
image = "registry.example/consumer@sha256:..."
run = "exec ./consume"
```

Without `health`, the worker has no Fly listener, origin, default `PORT`, or
public IP allocation. Its Machines API config and source-build `fly.toml` set
restart policy to `always`. Readiness checks the exact owned machine, deployment
receipt, configuration, image digest, and started state. A stopped or changed
machine fails the readiness step with `fly.worker.not_ready`; status does not
report a worker with configuration drift as ready; its stage is `drifted`. Dependencies on `ready` wait
for that native observation. Each up operation reconciles the recorded machine.
A stopped or suspended worker gets one journaled start request. The receipt
records the operation, machine, version, and deployment receipt before the POST.
Recovery observes an applied start; an unresolved start blocks another POST.
A changed definition cannot bypass an unresolved prior start.
See [Fly restart policies](https://fly.io/docs/machines/guides-examples/machine-restart-policy/).

A worker may declare HTTP `health`; that explicitly enables the same public
listener and origin as an HTTP service. Origin references, `root_origin`, and
endpoint declarations for a worker without HTTP health are rejected before
admission. Worker logs currently expose Fly machine lifecycle events, matching
the existing Fly service log source. Runtime stdout/stderr is not available
through that API. Fly finite jobs remain unsupported.

`depends_on` accepts `started`, `ready`, or `completed`. `completed` requires a
job. The engine schedules independent steps concurrently, with at most 16
running steps. Teardown follows dependency edges in reverse. A deletion failure
protects that resource's prerequisites while unrelated workloads can be removed.

## Provider placement

`--on` supplies the default hosting provider. A workload or resource can override
it with `on = "provider"`. The controller builds the selected adapters and runs
one dependency graph across them. This includes local/cloud combinations and
multiple cloud hosting providers. Resource `on` selects the hosting adapter;
its `provider` field still selects the catalog resource type.

```toml
[workloads.api]
on = "fly"
image = "nginx:alpine"
health = { path = "/" }

[workloads.api.fly]
internal_port = 80

[endpoints.api]
workload = "api"

[jobs.check]
on = "local"
run = 'curl --fail --silent --show-error "$API"'
env = { API = "${endpoints.api.url}" }
depends_on = { api = "ready" }
```

The complete [mixed fixture](../fixtures/mixed/stackless.toml) runs with:

```sh
stackless up --on local --name mixed --file fixtures/mixed/stackless.toml --confirm-paid --allow-host-execution
stackless down mixed
```

Cloud placement requires Stripe context even when the default is local. The
local job requires a host execution grant. Placement does not change an adapter's
supported workloads or network authority. A local isolated container cannot
reach a cloud endpoint; validation rejects that reference. `--source` pins are
checked against each selected adapter, including inherited pins on resume.

Local and Fly origins are available before startup. Other cloud origins come
from recorded deployments. URL dependencies use the target workload's adapter.
Readiness dependencies remain separate. A late URL reference to the same
workload is a cycle, including `${services.NAME.origin}`. Consumed URL changes
reconcile consumers; unrelated provider output does not enter their revisions.

Placement is recorded before execution and retained when a definition removes a
workload or resource. Teardown and logs use that record. A live checkpoint or an
unfinished resource prevents reassignment with `state.placement_conflict`.
Remove the old declaration and run `up` to retire it, or destroy the instance.
After verified teardown, the logical name can be placed on another adapter.

`up` and `check --on` return `placements.workloads` and `placements.resources`,
mapping logical names to hosting providers. Status entries include `on`.
Verification uses the selected provider of its source workload. Logs from a
provider without a log facility remain marked unavailable when another provider
returns logs. Mixed spend reports retain each provider's report and sum their
separate provider caps. The default lease is the shortest adapter default.

## TCP services

Local host processes support a TCP listener through `health.protocol`:

```toml
[workloads.database]
run = './database --listen "127.0.0.1:$PORT"'
health = { protocol = "tcp" }

[endpoints.database]
workload = "database"

[jobs.migrate]
run = './migrate "$DATABASE"'
env = { DATABASE = "${endpoints.database.url}" }
depends_on = { database = "ready" }
```

The process must listen on its injected `PORT`. The endpoint uses the port
saved with the process receipt: `tcp://127.0.0.1:<port>`. No HTTP proxy route is
created. Endpoint and native origin references wait for the start receipt.
A process cannot consume its own dynamically allocated URL during start.
Use `PORT` to configure its listener instead.

`ready` proves a TCP connection succeeds. It does not prove authentication,
a query, or another application protocol. Use a dependent job or verification
command for those checks. Each connection attempt has a 5s timeout; startup
retries for up to 300s and fails early if the process exits. Status reports
connection refusal as unready and other network failures as unknown.
Changing the listener's URL invalidates consumers that reference it.

TCP requires an explicit host-execution grant. Isolated containers and all
cloud adapters currently reject TCP service admission with `provider.unsupported`.
HTTP paths, status/body assertions, and `root_origin` do not apply to TCP.

`fixtures/tcp` contains a TCP echo server and a dependent protocol check:

```sh
stackless up --on local --name tcp-demo --file fixtures/tcp/stackless.toml --allow-host-execution
stackless down tcp-demo
```

## Named endpoints

An endpoint names a URL for a workload with `health`:

```toml
[endpoints.api]
workload = "web"

[endpoints.public-api]
workload = "web"
url = "https://api.example.com/v1"

[workloads.client.env]
API = "${endpoints.api.url}"
PUBLIC_API = "${endpoints.public-api.url}"
```

Without `url`, the endpoint uses the workload's provider-assigned origin.
With `url`, it uses the declared URL. HTTP workloads accept HTTP or HTTPS URLs,
including a path. TCP workloads accept `tcp://host:port`, with a nonzero port
and no path, query, or fragment. All URLs require a host and prohibit embedded
credentials. Multiple aliases can name the same
workload. They do not replace `${services.web.origin}`. Stackless does not create
DNS records, certificates, custom-domain bindings, or routes for declared URLs.
The caller must configure that routing separately.

A declared URL is available before startup. A provider-assigned URL requires
`start` when the provider cannot supply origins early. A dynamic reference to
one's own endpoint on such a provider is a dependency cycle. `depends_on` still
controls readiness or job completion, including when an explicit URL is known.
Changing a consumed endpoint declaration or provider URL reconciles its consumer.
Changes to unused endpoint aliases do not change the consumer's step revision.

`up` returns an `endpoints` map with `workload`, `url`, and `source` fields.
`source` is `provider` or `declared`. Status includes each declared endpoint,
its optional URL, source, and readiness. Provider endpoints inherit the workload's
observed readiness. Declared endpoints have unknown readiness because the
workload's health probe does not prove that a separate URL routes to it.

Rust, TypeScript, Python, and Go SDKs expose these bindings and URL maps for
`stackless bind` generated endpoint types. Verification environments accept
`${endpoints.<name>.url}` too. Integration configuration does not support endpoint
references and rejects them during validation. Exact URL references in non-sensitive env keys
remain public; previously recorded secrets still take precedence in redaction.
Container references resolve provider endpoints through the instance's private
network. A local container cannot reference a declared external endpoint or a
host-process endpoint on that isolated network.

`setup` initializes each materialized working copy. `prepare` runs for each
accepted up operation. Cloud materialization creates a new copy per operation,
so setup runs again even when the Git commit is unchanged.
Recovery of the same operation reuses its recorded hook or job execution. A
local process that disappears without an exit receipt produces
`job.result_unknown`. Recovery does not guess whether its effects happened.
`timeout_secs` defaults to 300 and must be between 1 and 86400.

Local shell jobs, setup, and prepare use owned `local-job` resources. A key
binds the instance, operation, and step. A separate digest binds the command,
working directory, resolved environment, and step revision. Changed inputs in
the same operation cannot launch another process. The process waits behind a
gate until its PID, start time, invocation cookie, and deadline are committed.
Its watchdog enforces the deadline even while the controller is dead.

Each execution retains the first 64 KiB of combined output in its private
workspace. `logs` reads this inventory, including failed commands that have no
successful checkpoint, and redacts saved secrets. Teardown checks the owner and
workspace marker, stops the command and helpers carrying its cookie, then removes
the workspace. Launch and teardown share a process lock. A watchdog timeout is
`job.timeout`; a command that explicitly exits 124 is `job.failed`.

Older local receipts remain recoverable without launching replacement commands.
They retain the old PID and start-time checks and shared service log path. They
have no independent watchdog or invocation cookie.

New long-running host services use an internal `stackless daemon workload`
process. It receives the command through its launch gate after the controller
commits its PID, start time, and invocation cookie. The runner and its log
collector keep running while the controller is dead. This uses the same CLI
binary as the controller. Embedded Rust controllers require the CLI on PATH or
`STACKLESS_BIN`; hermetic tests can pass a binary to `TestContext::with_cli`.

The collector writes short messages immediately and rotates at exactly 1 MiB.
It retains the current file and two older generations, at most 3 MiB per service.
`logs` reads the newest 64 KiB across these files and redacts recorded env values.
A writer lock prevents two runners from writing the same service log. Existing
large generations are capped before launching a replacement. Log paths must be
ordinary files inside the instance namespace. Container log snapshots remove
obsolete host generations before replacing the current file.

A write or rotation failure stops the service generation. Normal teardown sends
SIGTERM to its process group and gives it five seconds to exit. The collector
stays alive for shutdown output, with at most two seconds to drain after the
service exits. Teardown then kills any remaining processes carrying the exact
invocation cookie. A missing runner with surviving helpers is reported as drift,
so recovery replaces it instead of accepting the helpers as a healthy runner.
Retained logs remain until instance garbage collection. Empty writer lock files
remain while the namespace exists. Legacy running services keep their direct
log files and original process identity until replaced; their writes remain
unbounded and escaped helpers cannot be recovered through a cookie.

Verification uses the same gated process primitive through an owned
`verification-command` resource managed by the controller across providers.
`[stack.verify]` and each named tier accept `timeout_secs`, default 300, range
1 through 86400. Each accepted operation records one command per tier. Restart
reconnects to its receipt; cancellation stops the recorded process and helpers
carrying its cookie. Failed output is capped at 64 KiB, retained for `logs`, and
redacted on read. Teardown stops verification before removing its prerequisites.
Old verification operations without the journal-version event remain interrupted.

Cloud verification reuses an initialized source snapshot. Restoring a missing
working copy runs setup through a separate command receipt before verification.
A lost copy that this verification operation already initialized produces a source
error rather than rerunning setup. Legacy Render and Vercel source references
without a usable checkout materialize a journaled snapshot and initialize it
through the same command runner.

Cloud setup and prepare commands require the instance's stored host-execution grant, including
calls through provider adapters outside the CLI. Each command has an owned
`cloud-command` resource keyed by instance, operation, step, and provider. The
record includes an input digest, deadline, and process identity. The runner waits
behind a pipe gate until the process identity is committed. Recovery waits for
that process. Changed inputs within the same operation fail instead of launching
another command. A process lock serializes launch and teardown for this receipt.

The runner retains the first 64 KiB of combined output and drains the rest.
Its independent watchdog kills its process group at the recorded deadline.
Cancellation, completion, and teardown also stop detached helpers carrying the
invocation cookie. Teardown stops the command before removing its workspace,
source snapshot, or prerequisite integrations. Empty lock files remain in place.
The private command workspace is excluded from source archives. Missing exit
receipts remain unknown. A completed checkpoint needs a stopped process and an
exit code of zero before recovery can reuse it.

The standalone `run_prepare_command` helper has a 300-second deadline and bounded
output, but no durable resource inventory. Provider execution uses the journaled
snapshot runner. Native cloud jobs and detached helpers that
remove the invocation cookie still need separate execution contracts.

## Source scope

A source can declare `repo` and `ref`, a local `path`, or neither. `root` selects a
relative directory inside that source. A declared local path must stay inside
the definition directory. The caller can authorize another checkout with
`--source workload=PATH`. A definition cannot select arbitrary operator files
through a local Git URL.

Git materialization pins a commit for each operation. Different commits have
different checkout directories. Updating a branch does not rewrite a running
workload's directory. `--dirty` captures local source pins in separate directories
for each operation. Old owned sources remain in the inventory until teardown.

Render, Vercel, Netlify, Cloudflare, WordPress, GitLab, Fly, Railway, and Laravel
Cloud materialize a durable Git snapshot before prepare or source deployment. The resource inventory records the destination before
fetching. A sealed archive records the resolved commit and content digest before
the database marks the snapshot ready. Recovery reuses that archive if the branch
moves or the repository disappears. A new operation can resolve a newer commit.

Cloud uploads read the sealed archive. Prepare and verification use a separate
working copy extracted from the same snapshot. Hooks can install dependencies or
write files there without changing later upload bytes. The working copy excludes
Git metadata and protected credential files. `source.root` and the provider's
`root` are aliases for one directory. Either can supply it. When both are set,
validation requires them to agree after removing `.` path components and trailing
slashes. Absolute paths, parent traversal, and protected credential paths fail
validation. Snapshot materialization rejects a directory absent from the archive
before the provider provisions the workload.

Vercel uploads crop the archive to this directory and clear the deployment's
`rootDirectory` setting. Git deployments send the selected directory unchanged.
Both paths send the configured build settings, including explicit resets for
removed settings. Render sets `rootDir` and reads it back before submitting the
pinned deployment. The controller records that root for drift observation.

Render sends the saved commit to its deploy API. Vercel sends it as
[`gitSource.sha`](https://vercel.com/docs/rest-api/deployments/create-a-new-deployment).
Netlify static/ZIP uploads, Cloudflare uploads, WordPress HTML publication, and
GitLab Pages commits consume saved bytes. GitLab writes the target project's
default branch, independent of the upstream source ref. Commit actions use
base64 encoding for binary assets. Netlify
Git builds still follow a branch and do not yet satisfy this source contract.
Its [build API](https://open-api.netlify.com/#tag/build) exposes a branch selector.
GitLab preserves repository visibility during deployment. It filters pipelines by
returned commit SHA and target branch, then validates the pipeline's project, ID,
branch, and SHA on each status read. A failed pipeline cannot report a successful
deployment. GitLab must also return an active root Pages deployment; Stackless
no longer substitutes a URL inferred from the project path. New deployments include a reserved
`.well-known/stackless-deployment.json` receipt. Readiness and observation read
this file from the Pages endpoint without the GitLab API token. The response is
limited to 4096 bytes. A different serving receipt or branch tip reports drift.

New journaled GitLab deployments read the repository tree at a full branch SHA,
then replace `public/` and `.gitlab-ci.yml` in one commit. Files outside those
managed paths remain untouched. Removed assets become delete actions; updates
and deletes carry the file's observed `last_commit_id`. The
[commit API](https://docs.gitlab.com/api/commits/#create-a-commit) provides this
file-level conflict check. Tree reads paginate and reject duplicate paths,
malformed entries, and managed submodules. A missing branch only means an empty
repository when the project explicitly reports `empty_repo: true`. The returned
commit's file set is read back before deployment can become ready.

A completed deployment with a different serving receipt or branch tip starts a
new repair generation. The old request remains recorded, and the new generation
gets its own commit and serving receipt. A lost repair response resumes that
request after database reopen. An unfinished commit submission cannot start a
repair generation. This repairs drift after a completed deployment; retries of
failed CI pipelines and legacy requests without completion evidence remain
unfinished. File-level conflict checks do not provide a transaction with other
Git writers. Concurrent changes can still leave a request unresolved; they must
not be reported as ready.

GitLab records catalog creation and commit intent before submission. Lost commit
responses recover by exact receipt from commit inventory. Commit, pipeline, and
job IDs persist before later checks. Teardown removes Pages before requesting
project deletion, then retains the resource until native absence and Stripe
removal are observed. GitLab.com project retention can keep teardown pending for
30 days. Legacy records still require the separate ownership audit.

WordPress journals the catalog site's numeric `BLOG_ID` and returned `SITE_URL`
before native publication. Each revision gets a page slug and metadata receipt.
The [page creation API](https://developer.wordpress.com/docs/api/1.1/post/sites/$site/posts/new/)
accepts both fields. A lost create response recovers only a matching page with
that receipt, site ID, page type, and content. Missing or ambiguous evidence
cannot authorize another page creation. A saved page ID survives later failures.

WordPress readiness reads the page with `context=edit`, checks its content hash
and publication status, verifies the homepage setting, and fetches the public
homepage for the revision's HTML comment marker. That public request sends no
OAuth token. Responses are limited to 2 MiB, with a 300-second serving wait.
A theme that omits the page, private or coming-soon visibility, stripped content,
or a missing marker cannot report ready. Reconciliation repairs changed content
on a recorded page using the [page update API](https://developer.wordpress.com/docs/api/1.1/post/sites/$site/posts/$post_ID/).
Content and homepage writes have durable intent and readback, so an applied
update is not repeated after a lost response.

WordPress teardown saves native identity before cancelling the Stripe catalog
subscription. WordPress's own [site deletion client](https://github.com/Automattic/wp-calypso/blob/trunk/client/state/sites/actions.js)
rejects deletion while subscriptions remain active. After confirmed catalog
removal, Stackless deletes a still-present native site and requires a native
404. Catalog removal alone never clears native ownership. Failed deletion,
revoked credentials, or a retained site remains unresolved. A submitted native
delete is not repeated while the site stays visible. Legacy ownership, cleanup
of superseded pages, and live provider conformance remain required.

Fly extracts the sealed archive into an owned build context at `source.root`.
The Dockerfile is relative to that root, independent of its own parent directory.
Generated configuration and an isolated CLI home sit outside the uploaded context.
The [Fly deploy command](https://fly.io/docs/flyctl/deploy/) receives an explicit
configuration path, a controlled environment, and the app-scoped deploy token.
Builder failure messages redact the token and application environment values.
Image-only services without prepare hooks do not fetch unused repositories.
Fly saves the catalog receipt, native app ID, organization, and IP-allocation
intent before further mutations. Machine submission records include a receipt
carried in the submitted environment. A lost result recovers only one matching
machine. Missing or ambiguous receipts cannot authorize another submission.
Image revisions update the recorded machine with its observed version; source
revisions restrict flyctl to that machine. Readback checks the receipt, desired
configuration, image digest, started state, and absence of extra app machines.
The image digest and resulting configuration remain pinned for observation.
Application environment values live in the private CLI config, outside the build
context and command-line arguments. Source builds use the configured guest size.

The native journal records the builder's immutable owner, input fingerprint,
PID, process start time, invocation cookie, and deadline before releasing its
execution gate. Recovery waits for that process before accepting a machine
receipt. The runner keeps the first 64 KiB of combined output and drains the
rest. Its watchdog enforces `timeout_secs`, default 300, on the process group
even if the controller dies. Reconnection and cancellation also stop detached
helpers that retained the exact invocation cookie. A missing exit receipt never
authorizes another launch. Build files live under `.stackless-builds`, which
source uploads exclude. Teardown stops recorded builders and removes their
workspaces before credential-dependent native deletion.

Native teardown pins the app ID and organization before the force-delete call,
then requires a successful absence lookup before catalog removal. The absence
proof is durable, so catalog-removal recovery does not need a revoked deploy
token. Failed, malformed, or mismatched identity reads retain ownership. See the
[Fly app API](https://fly.io/docs/machines/api/apps-resource/) and
[machine API](https://fly.io/docs/machines/api/machines-resource/).

Tests kill a runner's parent, reconnect to a running fake flyctl after SQLite
reopen, and verify cancellation and cleanup before a native authorization error.
They do not kill the controller during a real Fly request. The local watchdog
covers its process group; detached helpers need a surviving controller to find
their cookies. Fly's remote builder resources and work continuing on the provider
still need ownership and cancellation support. Failed builds without a machine
receipt remain unresolved. Repair generations, legacy ownership audit,
credential lifetime behavior, and live provider conformance remain unfinished.

Railway resolves Git refs into the shared durable snapshot and sends the saved
SHA through `serviceInstanceDeployV2(commitSha: ...)`. Prepare uses a separate
working copy at `source.root`; its changes cannot replace the requested commit.
Railway fetches that commit through its GitHub integration. It does not upload
the filtered archive. Image services without prepare skip unused Git checkouts.
Source, root, start command, and variables are applied with the documented
service-instance arguments and read back before deployment. Variable replacement
uses `skipDeploys` so that write does not trigger another deployment. See the
[Railway service API](https://docs.railway.com/integrations/api/manage-services).

Railway records catalog creation and native mutation intent in the same owned
resource row. Project descriptions carry an instance receipt. Service creation
stores a reserved receipt variable and creates an empty service before applying
source and root settings. Returned project, service, domain, and deployment IDs
persist before later requests. Receipt recovery paginates project and service
inventory, checks parent IDs and workspace identity, and rejects ambiguous or
foreign candidates. A lost deployment response without an ID stays unknown and
blocks another submission, including from a later revision.

Settings and variable writes persist intent before submission and read back their
results. Applied writes are not repeated after restart. A recorded generated
domain is recovered only from the unique domain under the owned service and
environment. Missing or ambiguous inventory cannot authorize another create.
Readiness and checkpoint observation check native ownership, configuration
fingerprints, domain identity, deployment ID, commit metadata, and the sole active
deployment pointer. A different setting reports drift. Automatic repair after an
already submitted settings write has drifted remains unfinished.

Teardown records project deletion before the request and reads the full
`projects(includeDeleted: true)` inventory. Only the exact project with its
matching receipt, workspace, and parsed deletion timestamp proves removal.
A successful mutation result alone does not. Missing rows, GraphQL errors,
revoked credentials, and malformed responses retain ownership. The proof persists
before Stripe cancellation. Railway's
[API error contract](https://docs.railway.com/integrations/api) permits authorization
errors in HTTP 200 GraphQL responses. Live verification must establish whether
deleted-project tombstones remain visible to the operator token; an unavailable
tombstone leaves teardown unresolved.

Service URLs come from native domain readback. Legacy checkpoints retain their
previous observation and teardown paths pending ownership audit. Account-token
scope relative to the linked Stripe account, implicit deployments when connecting
a source, persistent Git triggers, config-file overrides, and image digest proof
remain required. The tests use simulated APIs and SQLite reopen. They do not
establish process-kill recovery or live provider conformance.

Laravel Cloud checks the catalog application's ID, name, repository, and root
before requesting a deployment. The common source repository and provider
repository name must agree. It selects the application's `defaultEnvironment`
relationship and checks that environment's parent application and configured
branch. A deployment must retain its ID, environment, branch, and full commit
hash across status reads. Success also requires the environment's current
Deployment relationship to name that deployment and its status to be running or
hibernating. Missing URL evidence fails; no application-name URL is substituted.

These checks do not pin a branch before deployment. Laravel's
[API initiation endpoint](https://laravel.com/cloud/docs/api/deployments/initiate-deployment)
has no commit selector. Its documented
[deploy hook](https://laravel.com/cloud/docs/deployments#deploy-hooks) accepts
`commit_hash`, but this adapter does not yet use that transport. The API exposes `root_directory` during application creation, but
not in its update request. The current Stripe catalog schema exposes neither
root nor source-control-provider selection. Root creation and changes, provider
host identity and immutable deployment remain required.

Laravel Cloud catalog creation and native deployment now share an ownership
record. The application ID is saved before native API calls. Deployment intent
is saved before POST, and a returned deployment ID is saved before polling.
Retries resume that ID and check the recorded environment, branch, and commit.
A POST with no returned ID stays unresolved and cannot trigger another POST,
including under a later revision. The initiation API has no request receipt;
listing a newly appeared deployment cannot prove which caller created it.

Native observation checks the application and the recorded deployment against
the environment's current-deployment pointer. Teardown recovers missing native
IDs from catalog outputs, saves deletion intent, and requires a native 404
before removing the Stripe registration. Failed or unconfirmed deletion retains
ownership. A submitted DELETE is not repeated while the application remains
visible. Operators must resolve that unknown outcome if the request never arrived.
Store-reopen tests cover lost catalog creation, deployment polling, unknown POST,
native deletion, and catalog removal responses. They do not establish actual
process-kill recovery or live provider conformance. Legacy catalog-only records
still need the ownership migration audit.

Laravel Cloud still needs migration to the snapshot helper. Hook
process recovery and cloud sandbox execution also remain required.
Netlify saved build-setting resets and provider configuration-file overrides
still need reconciliation. Render's catalog-created static site can build before
the native root update; that initial build path still needs correction.

Container source copies exclude `.git`, `.projects`, `.stackless.env`, `.env`,
`.env.*`, and common operator credential directories. Links and special files are
rejected. Copies are limited to 100000 entries, 512 MiB, and 100 directory levels.
The caller's checkout is never mounted into a container. Image-only workloads
retain the image's filesystem and default working directory.

## Host execution

Local workloads without `image` require the caller's `--allow-host-execution`
grant. The Rust builders expose `.allow_host_execution()`. TypeScript uses
`allowHostExecution`, Python uses `allow_host_execution`, and Go uses
`AllowHostExecution`.

The grant belongs to one immutable instance ID. Resume inherits it. Destroying
an instance and reusing its name does not transfer the grant. The application
file cannot grant host access. Operator-side cloud hooks and native verification
also require the grant.

Host execution gives the command the controller user's filesystem and process
authority. Child environments contain selected application inputs and a small
runtime allowlist. Controller credentials are not inherited. Environment
filtering does not make host execution a sandbox.

## Container boundary

Local `image` workloads use Docker. The current backend targets Docker 29 and was
verified against Docker 29.7.2. It runs as UID/GID 65532 with:

- A read-only root filesystem, no Linux capabilities, and no new privileges.
- A private PID namespace, 128-process limit, 512 MiB memory limit, and one CPU.
- A 64 MiB `/tmp` tmpfs and an optional copied application workspace.
- An internal bridge with isolated gateway mode and no outbound DNS forwarding.

Workloads can reach other container workloads in the same instance. They have no
default route to the host or Internet. `${services.NAME.origin}` resolves to the
peer's internal HTTP address inside a container. An HTTP service listens on
`PORT=8080`.

A separate nginx ingress container connects the isolated network to the
controller's loopback proxy. Its image is pinned by digest, its upstream address
is fixed, and its published port binds only to `127.0.0.1`. nginx receives no
application or controller credentials. This follows Docker's distinction between
[internal and isolated gateway networks](https://docs.docker.com/engine/network/port-publishing/).

Docker creation intent includes an exact name and immutable owner labels. The
returned container ID is stored before start. Recovery checks those labels before
adopting a container. Docker retains finite-job exit status. The controller
restores HTTP routes only after checking the recorded workload and ingress.

Container outbound Internet access and references to native host workloads are
not implemented. Container references to endpoints outside the isolated network
are rejected during validation. Cloud adapters advertise their own capabilities and reject
unsupported container images before provisioning.
