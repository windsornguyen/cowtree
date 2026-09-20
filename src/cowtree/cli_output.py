"""Serialize standalone command receipts without changing human-readable defaults."""

from dataclasses import asdict, dataclass
import json
import os
from typing import Literal

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import Command, DoctorReport, Worktree


@dataclass(frozen=True)
class Success:
    """One completed operation; unsupported doctor probes remain valid reports."""

    kind: Command
    value: Worktree | DoctorReport | None
    status: Literal["ok"] = "ok"

    def json(self) -> str:
        result = json.dumps(asdict(self), default=os.fspath, ensure_ascii=True)
        return result


@dataclass(frozen=True)
class Failure:
    """One typed failure emitted on stderr, never mixed with a success receipt."""

    code: CowtreeErrorCode
    message: str
    status: Literal["error"] = "error"

    @classmethod
    def from_error(cls, error: CowtreeError) -> "Failure":
        result = cls(code=error.code, message=error.message)
        return result

    def json(self) -> str:
        result = json.dumps(asdict(self), ensure_ascii=True)
        return result


def json_requested(argv: list[str]) -> bool:
    """Recognize structured diagnostics even when argument parsing cannot complete."""
    boundary = argv.index("--") if "--" in argv else len(argv)
    result = "--json" in argv[:boundary]
    return result
