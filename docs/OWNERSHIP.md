# Instance ownership

A display name identifies the current instance. Each birth also has a random
128-bit ID and a `sl-<id>` resource namespace. Reusing a display name after verified
teardown allocates another ID. Resource history remains attached to its original
owner.

Provider calls receive display name, resource namespace, and checkpoint handles
separately. New cloud resource names combine the namespace with a 64-bit SHA-256
suffix of the logical resource name. They are 52 bytes long, independent of stack
or service name length. Local public hostnames keep their display names; owned
source directories and logs use the resource namespace. Legacy provider names
remain unchanged until those instances are destroyed.

## Resource evidence

Inventory records distinguish owned resources from borrowed and shared references.
Only owned records authorize deletion. Creation intent precedes the side effect;
a returned handle is saved before configuration or deployment. Teardown also
reads unfinished records whose engine step never completed.

Dependency edges prevent parent deletion while a child survives. Instance-scoped
resources outlive every step resource. Teardown confirms absence before discarding
owned evidence. Tombstoning, name reuse, and garbage collection reject unresolved
resources.

## Stripe context

The controller stores shared Stripe project anchors by definition scope or explicit
project ID. A project creation name is persisted before `init`. After a lost result,
recovery uses exact-name inventory lookup. An ambiguous or missing result after
creation started does not authorize another create request. Shared project anchors
remain after instance teardown.

Each instance has a private runtime directory with mode 0700 and a definition
snapshot with mode 0600. Stripe project selection, environment activation, resource
calls, and vault reads use this directory under one session lock. The application
checkout supplies the `.stackless.env` overlay and is never a Stripe working
directory during lifecycle execution.

Stripe environments are owned inventory entries bound to a project ID and the
instance namespace. Their deletion is independently checked. Vault reads for a
named environment use only that environment's file. They do not merge credentials
from the combined `.env` file.

The reaper queues garbage collection through the controller. After the retention
window, it removes the expired birth's logs and private runtime. A delayed request
cannot target a later birth that reused the same display name. Terminal operation
inputs expire after seven days once their immutable birth is gone and its
resources are absent. An unbound failure waits until its alias has no instance or
pending work. Cleanup derives upload paths from the accepted request and removes
files before replacing its body with an idempotency digest. A restart can repeat
file deletion if the database update was lost.

## Remaining migration work

Local service launch and Stripe context recovery use these records. Render service
creation records partial progress. Integration provisioning and other cloud adapters
still need complete registration before each side effect. Legacy cloud records need
an ownership audit before their names can be treated as exclusive authority. Status,
credential output confinement, and workload sandboxing remain on the
[overhaul ledger](OVERHAUL.md).
