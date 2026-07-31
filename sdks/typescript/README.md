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

Calls block on the CLI via `spawnSync` (synchronous subprocess I/O). Methods return Promises for a stable async API, but each call completes before the Promise resolves.

## Protocol

See [../PROTOCOL.md](../PROTOCOL.md) for envelope shapes and verb mapping.

**Warning:** `up --json` success output may include integration credentials. Do not log raw envelopes in CI without redaction.
