from __future__ import annotations

import json
import os
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Sequence

from stackless.errors import StacklessError, error_from_envelope


def _default_bin() -> str:
    override = os.environ.get("STACKLESS_BIN", "").strip()
    return override if override else "stackless"


def _origins_map(raw: Any) -> dict[str, str]:
    if not isinstance(raw, list):
        return {}
    out: dict[str, str] = {}
    for item in raw:
        if isinstance(item, dict):
            svc = item.get("service")
            origin = item.get("origin")
            if isinstance(svc, str) and isinstance(origin, str):
                out[svc] = origin
    return out


@dataclass(frozen=True)
class SecretRef:
    kind: str
    instance_id: str
    integration: str
    output: str


def _secret_refs(raw: Any, instance_id: str) -> dict[str, dict[str, SecretRef]]:
    if not isinstance(instance_id, str) or not instance_id:
        raise StacklessError("up response lacks an immutable instance ID")
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise StacklessError("invalid integration reference map")
    out: dict[str, dict[str, SecretRef]] = {}
    for integration, values in raw.items():
        if not isinstance(integration, str) or not isinstance(values, dict):
            raise StacklessError("invalid integration reference map")
        out[integration] = {}
        for output, ref in values.items():
            expected = {"kind": "secret_ref", "instance_id": instance_id,
                        "integration": integration, "output": output}
            if not isinstance(output, str) or ref != expected:
                raise StacklessError("invalid or foreign integration secret reference")
            out[integration][output] = SecretRef(**expected)
    return out


@dataclass
class Create:
    on: str
    file: str | Path | None = None
    name: str | None = None
    sources: list[str] = field(default_factory=list)
    dirty: bool = False
    allow_host_execution: bool = False
    lease: str | None = None
    confirm_paid: bool = False


@dataclass
class Resume:
    name: str
    file: str | Path | None = None
    sources: list[str] = field(default_factory=list)
    dirty: bool = False
    allow_host_execution: bool = False
    lease: str | None = None


UpRequest = Create | Resume


@dataclass(frozen=True)
class EndpointBinding:
    workload: str
    url: str
    source: str


def _endpoint_bindings(raw: Any) -> dict[str, EndpointBinding]:
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise StacklessError("invalid endpoint binding map")
    bindings = {}
    for name, value in raw.items():
        if (not isinstance(value, dict)
                or not isinstance(value.get("workload"), str) or not value["workload"]
                or not isinstance(value.get("url"), str) or not value["url"]
                or value.get("source") not in ("provider", "declared")):
            raise StacklessError("invalid endpoint binding")
        bindings[name] = EndpointBinding(value["workload"], value["url"], value["source"])
    return bindings


@dataclass(frozen=True)
class Placements:
    workloads: dict[str, str] = field(default_factory=dict)
    resources: dict[str, str] = field(default_factory=dict)


def _placements(raw: Any) -> Placements:
    if raw is None:
        return Placements()
    if not isinstance(raw, dict):
        raise StacklessError("invalid placements")
    result = {}
    for kind in ("workloads", "resources"):
        values = raw.get(kind)
        if not isinstance(values, dict):
            raise StacklessError("invalid placement map")
        if any(not isinstance(name, str) or not name or not isinstance(on, str) or not on
               for name, on in values.items()):
            raise StacklessError("invalid hosting placement")
        result[kind] = dict(values)
    return Placements(**result)


@dataclass
class UpOutcome:
    instance: str
    instance_id: str
    substrate: str
    origins: dict[str, str]
    integrations: dict[str, dict[str, SecretRef]]
    executed: list[str] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)
    duration_ms: int = 0
    steps: list[Any] = field(default_factory=list)
    spend: Any | None = None
    endpoints: dict[str, EndpointBinding] = field(default_factory=dict)
    placements: Placements = field(default_factory=Placements)

    @property
    def endpoint_urls(self) -> dict[str, str]:
        return {name: endpoint.url for name, endpoint in self.endpoints.items()}


@dataclass
class DownOutcome:
    instance: str
    status: str
    spend: Any | None = None


@dataclass
class VerifyOutcome:
    instance: str
    duration_ms: int
    exit_status: int
    log_path: str
    tier: str | None = None
    lease_remaining_secs: int | None = None


@dataclass
class LogsOutcome:
    instance: str
    services: list[dict[str, Any]]
    substrate: str | None = None
    available: bool | None = None


@dataclass
class CheckOutcome:
    stack: str
    services: list[str]
    graph: dict[str, Any]
    substrate: str | None = None
    placements: Placements | None = None


Runner = Callable[[Sequence[str], str | None], subprocess.CompletedProcess[str]]


class Client:
    def __init__(
        self,
        bin: str | None = None,
        cwd: str | Path | None = None,
        *,
        run: Runner | None = None,
        controller: str | None = None,
    ) -> None:
        self._bin = bin or _default_bin()
        self._cwd = str(cwd) if cwd is not None else None
        self._run = run or _default_runner
        self._controller = controller

    @classmethod
    def system(cls, cwd: str | Path | None = None) -> Client:
        return cls(cwd=cwd)

    def _invoke(self, args: list[str]) -> dict[str, Any]:
        target = ["--controller", self._controller] if self._controller else []
        cmd = [self._bin, "--json", *target, *args]
        proc = self._run(cmd, self._cwd)
        text = (proc.stdout or "").strip()
        data: dict[str, Any] | None = None
        if text:
            try:
                parsed = json.loads(text)
                if isinstance(parsed, dict):
                    data = parsed
            except json.JSONDecodeError:
                pass
        if data is not None and data.get("ok") is False:
            raise error_from_envelope(data)
        if data is not None and data.get("ok") is True:
            return data
        detail = (proc.stderr or "").strip() or f"exit status {proc.returncode}"
        raise StacklessError(message=detail, code="cli_failed")

    @staticmethod
    def _up_args(request: UpRequest) -> list[str]:
        args = ["up"]
        if isinstance(request, Create):
            args.extend(["--on", request.on])
            if request.name:
                args.extend(["--name", request.name])
            if request.file is not None:
                args.extend(["--file", str(request.file)])
            for src in request.sources:
                args.extend(["--source", src])
            if request.allow_host_execution:
                args.append("--allow-host-execution")
            if request.dirty:
                args.append("--dirty")
            if request.lease:
                args.extend(["--lease", request.lease])
            if request.confirm_paid:
                args.append("--confirm-paid")
        else:
            args.extend(["--name", request.name])
            if request.file is not None:
                args.extend(["--file", str(request.file)])
            for src in request.sources:
                args.extend(["--source", src])
            if request.allow_host_execution:
                args.append("--allow-host-execution")
            if request.dirty:
                args.append("--dirty")
            if request.lease:
                args.extend(["--lease", request.lease])
        return args

    def up(self, request: UpRequest) -> UpOutcome:
        args = self._up_args(request)
        data = self._invoke(args)
        return UpOutcome(
            instance=str(data["instance"]),
            instance_id=data.get("instance_id"),
            substrate=str(data["substrate"]),
            origins=_origins_map(data.get("origins")),
            endpoints=_endpoint_bindings(data.get("endpoints")),
            placements=_placements(data.get("placements")),
            integrations=_secret_refs(data.get("integrations"), data.get("instance_id")),
            executed=list(data.get("executed") or []),
            skipped=list(data.get("skipped") or []),
            duration_ms=int(data.get("duration_ms") or 0),
            steps=list(data.get("steps") or []),
            spend=data.get("spend"),
        )

    def submit_up(self, request: UpRequest) -> dict[str, Any]:
        return self._invoke([*self._up_args(request), "--no-wait"])["operation"]

    def submit_down(self, name: str) -> dict[str, Any]:
        return self._invoke(["down", name, "--no-wait"])["operation"]

    def operation(self, operation_id: str, after: int = 0) -> dict[str, Any]:
        return self._invoke(["operation", "get", operation_id, "--after", str(after)])["result"]

    def cancel_operation(self, operation_id: str) -> dict[str, Any]:
        return self._invoke(["operation", "cancel", operation_id])["operation"]

    def wait_operation(self, operation_id: str) -> Any:
        return self._invoke(["operation", "wait", operation_id])["result"]

    def operations(self, instance: str | None = None) -> list[dict[str, Any]]:
        args = ["operation", "list"]
        if instance is not None:
            args.extend(["--instance", instance])
        return self._invoke(args)["result"]

    def down(self, name: str) -> DownOutcome:
        data = self._invoke(["down", name])
        status = data.get("outcome") or data.get("status") or ""
        return DownOutcome(
            instance=str(data.get("instance") or name),
            status=str(status),
            spend=data.get("spend"),
        )

    def verify(self, name: str, tier: str | None = None) -> VerifyOutcome:
        args = ["verify", name]
        if tier:
            args.extend(["--tier", tier])
        data = self._invoke(args)
        lease = data.get("lease_remaining_secs")
        return VerifyOutcome(
            instance=str(data.get("instance") or name),
            tier=data.get("tier"),
            duration_ms=int(data.get("duration_ms") or 0),
            exit_status=int(data.get("exit_status") or 0),
            log_path=str(data.get("log_path") or ""),
            lease_remaining_secs=int(lease) if lease is not None else None,
        )

    def status(self, name: str) -> dict[str, Any]:
        return self._invoke(["status", name])

    def list(self) -> dict[str, Any]:
        return self._invoke(["list"])

    def logs(
        self,
        name: str,
        service: str | None = None,
        *,
        tail: int | None = None,
    ) -> LogsOutcome:
        args = ["logs", name]
        if service:
            args.append(service)
        if tail is not None:
            args.extend(["--tail", str(tail)])
        data = self._invoke(args)
        substrate = data.get("substrate")
        available: bool | None = None
        if substrate is not None:
            available = False
        elif "available" in data:
            available = bool(data["available"])
        else:
            available = True
        return LogsOutcome(
            instance=str(data.get("instance") or name),
            substrate=str(substrate) if substrate is not None else None,
            available=available,
            services=list(data.get("services") or []),
        )

    def check(self, file: str | Path, on: str | None = None) -> CheckOutcome:
        args = ["check", str(file)]
        if on:
            args.extend(["--on", on])
        data = self._invoke(args)
        return CheckOutcome(
            stack=str(data["stack"]),
            substrate=data.get("substrate"),
            services=[str(s) for s in data.get("services") or []],
            graph=dict(data.get("graph") or {}),
            placements=_placements(data["placements"]) if data.get("placements") is not None else None,
        )


def _default_runner(cmd: Sequence[str], cwd: str | None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(cmd),
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
    )
