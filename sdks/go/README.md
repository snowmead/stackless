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

`up` returns secret references scoped to the immutable instance ID. Inject
credentials into service or verify environments with `${integrations.<name>.<output>}`.
The controller redacts known secrets from returned logs and errors.

## Tests

```bash
cd sdks/go && go test ./...
```

Named endpoint bindings are in `out.Endpoints`. Each entry has `Workload`,
`URL`, and `Source` (`provider` or `declared`). `out.EndpointURLs()` supplies the
map accepted by generated `BindEndpoints`. Declared URLs are unverified.
TCP workloads return `tcp://host:port`; endpoint URLs do not imply HTTP support.

Resolved hosting providers are in `outcome.Placements.Workloads` and the corresponding
`Resources` map. The top-level substrate is the default; individual workloads
and resources can select other providers with `on`.
