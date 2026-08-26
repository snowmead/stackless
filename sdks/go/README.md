# stackless (Go)

Go SDK for [stackless](https://github.com/snowmead/stackless). Speaks the JSON
protocol ([`../PROTOCOL.md`](../PROTOCOL.md)) via the `stackless` CLI.

## Module

```
github.com/snowmead/stackless/sdks/go
```

Publish tags use the subdirectory prefix, e.g. `sdks/go/v0.3.3` (lockstep with
the workspace version).

## Usage

```go
import "github.com/snowmead/stackless/sdks/go"

client := stackless.System()
out, err := client.Up(stackless.UpCreate(stackless.Create{
    On:   "local",
    File: "stackless.toml",
    Name: "demo",
}))
// out.Origins["web"], out.Integrations["clerk"]
_, _ = client.Down("demo")
```

Binary resolution: `STACKLESS_BIN`, then `stackless` on `PATH`.

Inject `ExecRunner` via `SetRunner` for tests.

## Secrets

`up --json` success envelopes may include integration credentials. Avoid logging
raw stdout in CI.

## Tests

```bash
cd sdks/go && go test ./...
```
