from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass
class StacklessError(Exception):
    message: str
    code: str = "unknown"
    remediation: str | None = None
    raw: dict[str, Any] | None = None

    def __str__(self) -> str:
        return self.message


def error_from_envelope(data: dict[str, Any]) -> StacklessError:
    err = data.get("error") or {}
    if isinstance(err, dict):
        return StacklessError(
            message=str(err.get("message") or "stackless command failed"),
            code=str(err.get("code") or "unknown"),
            remediation=err.get("remediation"),
            raw=err,
        )
    return StacklessError(message="stackless command failed", code="unknown")
