from __future__ import annotations

import json
from collections.abc import Callable, Sequence

import pytest

from stackless import Client, Create, Resume, StacklessError


def test_controller_is_forwarded_to_submissions_and_operation_reads():
    client = Client(bin="/fake/stackless", controller="ssh://deploy@builder", run=_runner({
        ("--controller", "ssh://deploy@builder", "down", "demo", "--no-wait"): {"ok": True, "operation": {"id": "op"}},
        ("--controller", "ssh://deploy@builder", "operation", "get", "op", "--after", "0"): {"ok": True, "result": {"operation": {"id": "op"}, "events": []}},
    }))
    assert client.submit_down("demo")["id"] == "op"
    assert client.operation("op")["operation"]["id"] == "op"


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
                    "instance_id": "owner-1",
                    "substrate": "local",
                    "executed": ["start:web"],
                    "skipped": [],
                    "duration_ms": 12,
                    "steps": [],
                    "origins": [{"service": "web", "origin": "http://demo.localhost:4444/"}],
                    "placements": {"workloads": {"web": "local", "api": "fly"}, "resources": {"clerk": "local"}},
                    "endpoints": {"public": {"workload": "web", "url": "https://public.example.test", "source": "declared"}},
                    "integrations": {
                        "clerk": {"secret_key": {"kind":"secret_ref", "instance_id":"owner-1", "integration":"clerk", "output":"secret_key"}}
                    },
                }
            }
        ),
    )
    out = client.up(Create(on="local", name="demo"))
    assert out.placements.workloads["api"] == "fly"
    assert out.placements.resources["clerk"] == "local"
    assert out.instance == "demo"
    assert out.endpoints["public"].source == "declared"
    assert out.endpoint_urls == {"public": "https://public.example.test"}
    assert out.origins["web"] == "http://demo.localhost:4444/"
    assert out.integrations["clerk"]["secret_key"].instance_id == "owner-1"


def test_up_resume():
    client = Client(
        bin="/fake/stackless",
        run=_runner(
            {
                ("up", "--name", "demo"): {
                    "schema_version": 1,
                    "ok": True,
                    "instance": "demo",
                    "instance_id": "owner-1",
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


@pytest.mark.parametrize("allowed", [False, True])
@pytest.mark.parametrize("kind", ["create", "resume"])
def test_host_execution_requires_explicit_caller_flag(allowed, kind):
    def run(cmd, cwd):
        assert ("--allow-host-execution" in cmd) is allowed

        class Proc:
            stdout = json.dumps({"ok": True, "instance": "demo", "instance_id": "owner-1", "substrate": "local", "origins": []})
            stderr = ""
            returncode = 0

        return Proc()

    request = Create(on="local", allow_host_execution=allowed) if kind == "create" else Resume(name="demo", allow_host_execution=allowed)
    Client(bin="/fake/stackless", run=run).up(request)


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


def test_rejects_plaintext_and_foreign_references():
    from stackless.client import _secret_refs
    for ref in ["sk_test_CANARY", {"kind":"secret_ref", "instance_id":"foreign", "integration":"clerk", "output":"secret_key"}]:
        with pytest.raises(StacklessError, match="invalid or foreign integration secret reference"):
            _secret_refs({"clerk":{"secret_key":ref}}, "owner-1")

@pytest.mark.parametrize("endpoints", [[], "url", {"public": {"workload": "web", "url": "https://example.test", "source": "invented"}}, {"public": {"url": "https://example.test", "source": "provider"}}])
def test_invalid_endpoint_bindings(endpoints):
    client = Client(bin="/fake/stackless", run=_runner({("up", "--name", "demo"): {
        "ok": True, "instance": "demo", "instance_id": "owner-1", "substrate": "local", "endpoints": endpoints,
    }}))
    with pytest.raises(StacklessError, match="invalid endpoint binding"):
        client.up(Resume(name="demo"))


def test_generated_endpoint_binding_names():
    import importlib.util
    import sys
    from pathlib import Path
    path = Path(__file__).resolve().parents[3] / "crates/stackless-idl/testdata/endpoints.py"
    spec = importlib.util.spec_from_file_location("test_endpoint_bindings", path)
    bindings = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = bindings
    spec.loader.exec_module(bindings)
    client = Client(bin="/fake/stackless", run=_runner({("up", "--name", "demo"): {
        "ok": True, "instance": "demo", "instance_id": "owner-1", "substrate": "local", "endpoints": {
            "native-api": {"workload": "web", "url": "http://native.example.test", "source": "provider"},
            "public-api": {"workload": "web", "url": "https://public.example.test/v1", "source": "declared"},
        },
    }}))
    outcome = client.up(Resume(name="demo"))
    endpoints = bindings.bind_endpoints(outcome.endpoint_urls)
    assert endpoints.native_api == "http://native.example.test"
    assert endpoints.public_api == "https://public.example.test/v1"
    with pytest.raises(bindings.BindError, match="missing URL for endpoint"):
        bindings.bind_endpoints({})


@pytest.mark.parametrize("placements", [[], "local", {}, {"workloads": {"api": False}, "resources": {}}, {"workloads": {}, "resources": {"db": ""}}])
def test_invalid_placements(placements):
    client = Client(bin="/fake/stackless", run=_runner({
        ("up", "--on", "local"): {"ok": True, "instance": "demo", "instance_id": "owner-1", "substrate": "local", "origins": [], "placements": placements},
    }))
    with pytest.raises(StacklessError, match="invalid .*placement"):
        client.up(Create(on="local"))
