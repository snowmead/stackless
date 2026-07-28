# stackless (Python)

CLI-backed SDK for [`stackless`](https://github.com/snowmead/stackless). Spawns the
`stackless` binary with `--json` and parses stdout envelopes (see
[`../PROTOCOL.md`](../PROTOCOL.md)).

## Install

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

Successful `up --json` stdout may include integration credentials. Do not log raw
JSON in CI without redaction.

## Tests

```bash
cd sdks/python && python -m pytest
```
