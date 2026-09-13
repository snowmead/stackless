# stackless-sdk (Python)

Python SDK for [`stackless`](https://github.com/snowmead/stackless). Published as
[`stackless-sdk`](https://pypi.org/project/stackless-sdk/) on PyPI
(`import stackless`). Speaks the JSON protocol
([`../PROTOCOL.md`](../PROTOCOL.md)) via the `stackless` CLI.

The PyPI distribution is **`stackless-sdk`**; the import package remains
`stackless`.

## Install

```bash
pip install stackless-sdk
```

Editable (from this repo):

```bash
pip install -e "sdks/python[dev]"
```

## Usage

```python
from stackless import Client, Create, Resume

client = Client.system()
out = client.up(Create(on="local", file="stackless.toml", name="demo"))
print(out.origins["web"])
print(out.integrations.get("clerk", {}))
client.down("demo")
```

Binary resolution: `STACKLESS_BIN`, then `stackless` on `PATH`.

## Secrets

`up` returns secret references scoped to the immutable instance ID. Inject
credentials into service or verify environments with `${integrations.<name>.<output>}`.
The controller redacts known secrets from returned logs and errors.

## Tests

```bash
cd sdks/python && python -m pytest
```

Named endpoint bindings are in `out.endpoints`. Each entry has `workload`,
`url`, and `source` (`provider` or `declared`). `out.endpoint_urls` supplies the
map accepted by generated `bind_endpoints`. Declared URLs are unverified.
TCP workloads return `tcp://host:port`; endpoint URLs do not imply HTTP support.

Resolved hosting providers are in `outcome.placements.workloads` and the corresponding
`resources` map. The top-level substrate is the default; individual workloads
and resources can select other providers with `on`.
