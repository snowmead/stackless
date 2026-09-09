# stackless-sdk

TypeScript SDK for [stackless](https://github.com/snowmead/stackless). Published
as [`stackless-sdk`](https://www.npmjs.com/package/stackless-sdk) on npm. Speaks
the JSON protocol (`../PROTOCOL.md`) via the `stackless` CLI.

## Install

```bash
npm install stackless-sdk
```

Requires `stackless` on `PATH`, or set `STACKLESS_BIN` to the binary path.

## Usage

```ts
import { Client } from "stackless-sdk";

const client = Client.system();
const outcome = await client.up({
  kind: "create",
  name: "demo",
  on: "local",
  file: "stackless.toml",
});

console.log(outcome.origins.web);
console.log(outcome.integrations);
```

Calls use asynchronous subprocess I/O. The Node event loop remains available while the CLI waits. `submitUp` and `submitDown` return durable operation IDs; `operation`, `waitOperation`, `cancelOperation`, and `operations` reconnect to the controller.

## Protocol

See [../PROTOCOL.md](../PROTOCOL.md) for envelope shapes and verb mapping.

`up` returns secret references scoped to the immutable instance ID. Inject
credentials into service or verify environments with `${integrations.<name>.<output>}`.
The controller redacts known secrets from returned logs and errors.

Named endpoint bindings are in `outcome.endpoints`. Each entry has `workload`,
`url`, and `source` (`provider` or `declared`). Import `endpointUrls` to build the
URL map accepted by generated `bindEndpoints`. Declared URLs are unverified.
TCP workloads return `tcp://host:port`; endpoint URLs do not imply HTTP support.

Resolved hosting providers are in `outcome.placements.workloads` and the corresponding
`resources` map. The top-level substrate is the default; individual workloads
and resources can select other providers with `on`.
