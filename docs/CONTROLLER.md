# Controller operations

The controller owns lifecycle execution for local and cloud instances. CLI,
Rust SDK, language SDKs, MCP, and the lease reaper submit to the same service.
The Unix socket and SQLite file have mode 0600. An OS file lock excludes a
second controller before either process opens lifecycle state.

## Submission and reconnect

```sh
stackless up --name demo --on local --no-wait --json
stackless operation get OPERATION_ID --after 0 --json
stackless operation wait OPERATION_ID --json
stackless operation list --instance demo --json
stackless down demo --no-wait --json
```

Submission persists the normalized request before returning an ID. For a local controller, the client
captures definition text and absolute source paths. Remote submissions contain
definition text and source archives, persisted before extraction or execution. Repeating the same ID and
request returns the same operation. Reusing an ID for another request fails.
The Rust SDK exposes `submit_up_with_id` for caller-owned idempotency keys.

The scheduler runs at most eight operations concurrently, with one running
operation per instance name. Progress events have durable sequence numbers.
`get --after` returns up to 256 events; pass the last sequence on the next call.
`wait` replays the events and returns the stored result or structured error.
Client exit or socket disconnect does not cancel execution.

## Cancellation and restart

`operation cancel ID` cancels queued work. For running `up`, it records a
cancellation request which the engine and owned command runners check.
Running journaled verification also accepts cancellation and stops its recorded
command and helpers carrying its invocation cookie. Resources and failed output
remain recorded for `down` to remove. Running `down` cannot be cancelled.

After controller death, unfinished up/down requests are queued with their
original operation IDs. Provider recovery must use persisted resource handles.
Verification publishes a journal-version event before execution. After restart,
these operations reconnect to their saved command receipt. A pending cancellation
is delivered to a worker so it can stop the process before recording cancellation
as complete. Older verification operations without that event become
`interrupted`; their unjournaled commands cannot be resumed safely.

Verification commands have a deadline that runs outside the controller and retain
at most 64 KiB of combined output. `timeout_secs` defaults to 300 for the default
proof and each named tier. The Rust client exposes `submit_verify` for callers
that need the operation ID before waiting.

The reaper submits down operations to the same queue. It defers instances with
queued or running work, and preserves teardown failure backoff. Tombstone garbage
collection uses internal `gc` operations in the same queue. Each request carries
the expired birth ID, so delayed cleanup cannot remove a reused name's files.

## Remote controller

```sh
stackless --controller ssh://controller-host controller --json
stackless --controller ssh://controller-host up --name demo --on local --file stackless.toml --json
stackless --controller ssh://controller-host down demo --json
```

`STACKLESS_CONTROLLER=ssh://controller-host` selects the same transport. Rust
uses `Client::builder().remote(...)`; TypeScript and Python accept `controller`,
and Go exposes `WithController`. CLI, SDK, and MCP calls use the selected owner.

OpenSSH authenticates the account and verifies the host key. Batch mode and
strict host checking prohibit interactive prompts and automatic host-key trust.
Configure keys, ports, and aliases in OpenSSH config. The fixed remote command
is `stackless daemon remote-control`. It connects to the account's running
controller; it does not start a temporary daemon. A request has a 30-second
transport timeout. An uncertain submission can be replayed with its operation ID.

Local source overrides and `source.path` become immutable uploads. The combined
limit is 64 MiB and 100000 entries. Symlinks, special files, traversal paths,
`.git`, environment files, and credential directories are rejected or excluded.
Source archives enter the durable operation request before extraction. Hooks and
workloads execute from owned copies. Teardown removes the instance's uploaded
snapshot. The controller expires the private request body after the operation has
been terminal for seven days and its instance birth has been collected. It also
removes partial uploads from failures before admission, once that alias has no
instance or queued work. Request digests, IDs, events, and results remain, so
repeating a retired request returns its original result. Active births retain
their inputs. SQLite reuses freed pages; this does not securely erase backups or
shrink the database file. Collection runs at controller startup and on the
operator's 60-second reaper tick, in batches of up to 100 requests. Use Git sources
when local archives exceed the limit.

Install the [systemd user service](../deploy/systemd/README.md) on a Linux host
for unattended lease expiry. `stackless controller --json` reports the selected
controller's reaper and persistence state. The systemd check requires its live
PID, an enabled unit, unconditional restart without rate limiting, and user
lingering. macOS checks launchd registration. Embedded controllers report no
persistent lease reaper.

## Current limits

The direct database driver has been removed. `STACKLESS_STATE_URL` is rejected
because a shared database does not supply one owner. Legacy SQL exports can be
[migrated into local SQLite](AGENT-FLEETS.md#migrate-a-legacy-fleet-database). The systemd deployment has not been verified
on a running Linux host. SSH argument and framing tests plus a real stdio bridge,
controller restart, upload, and teardown test cover the transport implementation.
They do not prove SSH login or reboot survival on a deployment host.

Recovery covers durable local service launch, shared Stripe project creation,
Stripe environments, and Stripe catalog creation/removal by remote resource ID.
Remaining hosting adapters still need complete side-effect recovery. Cancellation
is bounded by step completion. Track the remaining work in [OVERHAUL.md](OVERHAUL.md).
