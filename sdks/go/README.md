# stackless (Go)

CLI-backed SDK for [stackless](https://github.com/snowmead/stackless). Runs
`stackless --json …` and parses stdout envelopes ([`../PROTOCOL.md`](../PROTOCOL.md)).

## Module

```
github.com/snowmead/stackless/sdks/go
```

## Usage

```go
import "github.com/snowmead/stackless/sdks/go/stackless"

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
