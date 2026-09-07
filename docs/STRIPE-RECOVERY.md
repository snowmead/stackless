# Stripe resource recovery

Local Stripe registration is not remote deletion evidence. Projects 0.35 catches
some status refresh failures and can return cached rows. It also excludes failed
resources while reconciling local state. Stackless reads the remote provisioning
resource directly before reporting presence or absence.

The controller records the immutable owner, Stripe project binding, requested
name, catalog reference, and submission state before `projects add`. It saves
`data.service.key` and `data.service.provider_id` from the creation response before
environment attachment. A lost response triggers remote project inventory lookup.
The name must have exactly one match with the expected catalog identity. Empty,
malformed, repeated, or unavailable pages cannot authorize another creation.

Removal records submission before the remote request. The controller reads the
exact remote ID and checks its name, provider, and service reference before sending
`POST .../resources/<id>/remove`. A separate status read must return `removed`
before the engine marks the inventory entry absent. `pending` and `error` retain
the entry. A failed resource can still be removed. Borrowed and shared records
never authorize deletion.

All structured command failures remain failures. There is no plaintext retry.
Projects' interactive removal path can clean up shared plans; the controller must
not enter that path. Legacy records without a remote identity remain unresolved
until their ownership and remote handles are audited.

## Subprocess capture

The Stripe CLI runner retains at most 4 MiB of stdout and 256 KiB of stderr.
Exceeding either limit stops the invocation and returns
`stripe.projects.output_limit`, including when the command exits zero. Partial
output is never passed to the JSON parser. A valid empty-inventory prefix cannot
become deletion evidence when later bytes exceed the limit.

Pipe reads use nonblocking descriptors. After process cleanup, readers have a
shared two-second deadline to reach EOF. Each reader is stopped and joined if
the deadline expires. An incomplete pipe or read error returns
`stripe.projects.output_unavailable`. These errors omit captured bytes and
configuration arguments.

The internal `stackless daemon helper` process owns the 90-second command
budget, output capture, and process cleanup. It continues enforcing that budget
after controller death. Launch inputs record the executable, raw argument bytes,
and budget. The explicit command builder captures inherited env at construction
and preserves `env_clear`, removals, overrides, and the working directory.
Privileged Stripe credentials reach the provider CLI through this environment.

The helper acquires the per-directory Stripe command lock before acknowledging
its launch gate. The caller releases the command only after that acknowledgement.
Caller death before release cannot run the command. Caller death afterward leaves
the helper holding the lock through command termination and capture. A competing
command waits or returns `stripe.projects.lock_held`. Runtime snapshot replacement
waits for this lock; garbage collection defers while it is held.

Cleanup matches the exact `STACKLESS_SPAWN` environment entry and checks each
PID's recorded start time. It suspends owned processes in parent-first order
before killing them, so killing a sleeping child cannot wake its shell into the
next command. Cleanup rescans matching helpers for up to three seconds and returns
`stripe.projects.cleanup_failed` if any remain.

Results use a versioned pipe envelope with a 6 MiB transport cap. A malformed,
truncated, or missing envelope cannot produce command output. Output limits still
apply to the decoded streams. The helper creates no output files. CLI resolution
is shared with the daemon; embedded callers can provide an executable explicitly.
All `launchctl`, `systemctl`, and `loginctl` calls use the same helper with a
five-second command budget. Real-user ID lookup reads the OS directly.

These process deadlines do not undo a provider request already accepted remotely.
Lost responses remain unresolved through the resource journal. A helper killed
independently of its controller, or descendants that evade both recorded ancestry
and the invocation cookie, still require stronger containment or recovery.

## API compatibility

This transport follows the implementation shipped in Stripe Projects 0.35.0.
It uses `stripe get` and `stripe post` with `--live` and
`--stripe-version unsafe-development`. The route prefix is
`/v2/provisioning/internal/`. Credentials come from the existing Stripe CLI login
or its environment. Stackless does not export them into workload environments.

These are internal provisioning routes, not a stable public API guarantee.
An incompatible response fails closed. The driver requires complete resource
identity fields and follows only resource-list pagination routes on Stripe's API.
HTTP failures, including not-found errors, do not mean deprovisioning completed.

A read-only live probe on 2026-09-06 confirmed the project/resource list envelopes,
pagination fields, and resource `id`, `name`, `provider`, `service_ref`, and `status`
fields. It read a resource whose status was `removed`. It did not create or delete
resources. Hermetic tests cover lost responses, store reopen, hidden local state,
remote collisions, pagination failures, and unresolved deletion status. Live
creation and deletion conformance across providers remains required.

Stripe documents default resource deprovisioning separately from credential-only
unlinking in [Remove a service](https://docs.stripe.com/projects#remove-a-service).

## Vercel deployment receipts

Vercel project creation uses the catalog journal. Each native deployment is a
child resource with a receipt derived from the instance, step, project, and
desired revision. The receipt is persisted before POST and sent in deployment
metadata. A lost response is recovered by scanning the project's deployment
pages. A missing or ambiguous receipt remains unresolved.

The adapter records the returned deployment ID before build polling. Teardown
checks the deployment's project and receipt, records removal submission, sends
DELETE, and requires an independent GET to report absence. The project remains
owned until all deployment children are absent.

The API contract follows Vercel's [deployment creation](https://vercel.com/docs/rest-api/deployments/create-a-new-deployment),
[paginated inventory](https://vercel.com/docs/rest-api/deployments/list-deployments),
and [deletion](https://vercel.com/docs/rest-api/deployments/delete-a-deployment) endpoints.
The integration test uses local HTTP mocks. No live Vercel recovery run has
verified this implementation.

## Cloudflare script ownership

Workers account enablement is shared. Removing one instance cannot authorize
account deletion. Each native Worker has an owned resource record, an immutable
instance tag, and a desired revision in its upload metadata. The controller saves
submission before PUT. Recovery requires the same owner and revision from the
script settings endpoint. A missing submitted script remains unresolved.

Teardown checks the owner tag before DELETE and independently reads settings
afterward. A lost delete response can be recovered without another DELETE.
Foreign tags, failed authentication, malformed responses, and unresolved uploads
block cleanup. The shared catalog registration remains after instance teardown.

The wire fields follow Cloudflare's [multipart upload metadata](https://developers.cloudflare.com/workers/configuration/multipart-upload-metadata/)
and [Workers scripts API](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/).
Tests cover lost responses, store reopen, and two instances sharing one account.
No live Cloudflare create/delete run has verified these contracts.

## Render service and deployment receipts

Render service creation uses the catalog journal. The native service ID is saved
before environment configuration and deployment. Each deployment attempt records
its desired revision, Git commit, and the complete existing deployment inventory.
Auto-deploy is disabled before this snapshot. Submission is saved before POST.

A 201 response records the returned deployment ID immediately. For a queued or
lost response, recovery requires exactly one new API deployment with the recorded
commit. Empty inventory, multiple new deployments, a different trigger, or a
different commit remain unresolved. The controller never retries that POST.
This recovery assumes the controller is the only API deployment writer for its
owned service. Concurrent external deployments with the same commit cannot be
distinguished by Render's deployment response fields.

Readiness polls the recorded ID. A later deployment cannot replace it. Teardown
persists native removal before DELETE, checks the exact service ID and name, and
requires native absence before removing the Stripe registration. Both absence
observations must succeed before the inventory entry is retired.

The API contract follows Render's [deployment trigger](https://api-docs.render.com/reference/create-deploy),
[deployment inventory](https://api-docs.render.com/reference/list-deploys),
[pagination](https://api-docs.render.com/reference/pagination), and
[service updates](https://api-docs.render.com/reference/update-service).
The engine test uses local HTTP and Git fixtures. It does not establish live
provider conformance or recovery under an external concurrent deployment writer.

## Netlify site and deployment receipts

The owned catalog site's payload contains native site identity, creation and
removal submission flags, and every deployment attempt. A request's receipt is
derived from the immutable instance, catalog key, desired step revision, and
deploy mode. It is saved before POST and sent as the `title` query parameter.
File uploads, ZIP builds, and Git builds all follow this rule.

A build response saves its build ID and deployment ID before polling. Queued
builds with only a build ID recover through that ID. A lost response scans the
site's complete deployment inventory for exactly one receipt, then independently
reads the matching deployment. Missing, ambiguous, malformed, or foreign results
cannot authorize another POST. Static uploads resume the recorded deployment's
required SHA1 digests. ZIP bytes use sorted files and fixed timestamps.

When Stripe omits the native site ID, direct creation checks the complete site
inventory, records submission, and saves the returned ID. A lost response can
recover by the unique instance-specific name. A site present before submission
is an ownership conflict and blocks creation and teardown pending an audit.

Teardown retains the entire native ledger, records removal before DELETE, and
requires an independent read of the exact site ID and name to establish absence.
Catalog removal follows native absence. A failed delete response retains cleanup
information. The native site is the deletion unit for its builds and deployments.

Netlify documents the wire fields in its [OpenAPI specification](https://github.com/netlify/open-api/blob/master/swagger.yml)
and the file digest protocol in its [API guide](https://docs.netlify.com/api-and-cli-guides/api-guides/get-started-with-api/).
Git builds still use the configured branch. Static and ZIP retries read the
operation's sealed source archive. Prepare and verification use a working copy
of that same snapshot. Hook process recovery and cloud sandbox parity remain
required.
Live provider recovery and shared provider conformance remain unverified.
