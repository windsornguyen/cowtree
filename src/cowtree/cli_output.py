"""Serialize standalone command receipts without changing human-readable defaults."""

from dataclasses import dataclass
from typing import Literal

from pydantic import TypeAdapter

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import Command, DoctorReport, Worktree


@dataclass(frozen=True)
class Success:
    """One completed operation; unsupported doctor probes remain valid reports."""

    kind: Command
    value: Worktree | DoctorReport | None
    status: Literal["ok"] = "ok"

    def json(self) -> str:
        result = TypeAdapter(Success).dump_json(self).decode()
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
        result = TypeAdapter(Failure).dump_json(self).decode()
        return result


def json_requested(argv: list[str]) -> bool:
    """Recognize structured diagnostics even when argument parsing cannot complete."""
    boundary = argv.index("--") if "--" in argv else len(argv)
    result = "--json" in argv[:boundary]
    return result
