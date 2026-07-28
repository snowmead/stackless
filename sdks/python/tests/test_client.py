from __future__ import annotations

import json
from collections.abc import Callable, Sequence

import pytest

from stackless import Client, Create, Resume, StacklessError


def _runner(responses: dict[tuple[str, ...], dict]) -> Callable[[Sequence[str], str | None], object]:
    def run(cmd: Sequence[str], cwd: str | None):
        key = tuple(cmd[2:])
        payload = responses[key]

        class Proc:
            stdout = json.dumps(payload)
            stderr = ""
            returncode = 0 if payload.get("ok") else 1

        return Proc()

    return run


def test_up_create_maps_origins_and_integrations():
    client = Client(
        bin="/fake/stackless",
        run=_runner(
            {
                (
                    "up",
                    "--on",
                    "local",
                    "--name",
                    "demo",
                ): {
                    "schema_version": 1,
                    "ok": True,
                    "instance": "demo",
                    "substrate": "local",
                    "executed": ["start:web"],
                    "skipped": [],
                    "duration_ms": 12,
                    "steps": [],
                    "origins": [{"service": "web", "origin": "http://demo.localhost:4444/"}],
                    "integrations": {
                        "clerk": {"secret_key": "sk_test", "publishable_key": "pk_test"}
                    },
                }
            }
        ),
    )
    out = client.up(Create(on="local", name="demo"))
    assert out.instance == "demo"
    assert out.origins["web"] == "http://demo.localhost:4444/"
    assert out.integrations["clerk"]["secret_key"] == "sk_test"


def test_up_resume():
    client = Client(
        bin="/fake/stackless",
        run=_runner(
            {
                ("up", "--name", "demo"): {
                    "schema_version": 1,
                    "ok": True,
                    "instance": "demo",
                    "substrate": "local",
                    "executed": [],
                    "skipped": ["start:web"],
                    "duration_ms": 1,
                    "steps": [],
                    "origins": [],
                }
            }
        ),
    )
    out = client.up(Resume(name="demo"))
    assert out.skipped == ["start:web"]
    assert out.integrations == {}


def test_error_envelope():
    client = Client(
        bin="/fake/stackless",
        run=_runner(
            {
                ("down", "missing"): {
                    "ok": False,
                    "error": {
                        "code": "instance_not_found",
                        "message": "no such instance",
                        "remediation": "stackless list",
                    },
                }
            }
        ),
    )
    with pytest.raises(StacklessError) as exc:
        client.down("missing")
    assert exc.value.code == "instance_not_found"


def test_list_and_check():
    client = Client(
        bin="/fake/stackless",
        run=_runner(
            {
                ("list",): {
                    "schema_version": 1,
                    "ok": True,
                    "instances": [],
                    "persistence_warning": "leases ephemeral",
                },
                ("check", "stackless.toml"): {
                    "schema_version": 1,
                    "ok": True,
                    "stack": "demo",
                    "services": ["web"],
                    "graph": {"nodes": []},
                },
            }
        ),
    )
    listed = client.list()
    assert listed["instances"] == []
    checked = client.check("stackless.toml")
    assert checked.stack == "demo"
    assert checked.graph == {"nodes": []}
