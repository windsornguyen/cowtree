"""Typed Python access to the required libcowtree engine.

The compiled extension owns validation, locking, native cloning, and rollback.
Python decodes its records into the existing public dataclasses. A missing
extension is an installation error, never a request to run another engine.
"""

from collections.abc import Callable
from functools import partial
import json
from pathlib import Path
from typing import TypeVar

from pydantic import TypeAdapter

from cowtree import _libcowtree
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import DoctorReport, Worktree, WorktreeAddRequest


Value = TypeVar("Value")
ERROR = TypeAdapter(tuple[str, str])
WORKTREE = TypeAdapter(Worktree)
WORKTREES = TypeAdapter(list[Worktree])
DOCTOR = TypeAdapter(DoctorReport)


def invoke(operation: Callable[[], Value]) -> Value:
    """Translate only the extension's declared error record."""
    try:
        result = operation()
    except _libcowtree.EngineError as error:
        code, message = ERROR.validate_python(error.args)
        if code == "cancelled":
            raise KeyboardInterrupt(message) from error
        raise CowtreeError(code=CowtreeErrorCode(code), message=message) from error
    return result


def add_worktree(request: WorktreeAddRequest) -> Worktree:
    """Return the registered worktree only after native validation completes."""
    record = invoke(operation=partial(_libcowtree.add_worktree, request=request))
    result = WORKTREE.validate_python(json.loads(record))
    return result


def list_all_worktrees(source: Path | None = None) -> list[Worktree]:
    """Read the repository's worktrees through the shared native engine."""
    record = invoke(operation=partial(_libcowtree.list_worktrees, source=source))
    result = WORKTREES.validate_python(json.loads(record))
    return result


def remove_worktree(path: Path, *, source: Path | None = None, force: bool = False) -> None:
    """Retire the selected worktree while preserving its branch."""
    invoke(operation=partial(_libcowtree.remove_worktree, path=path, source=source, force=force))


def inspect_path(path: Path) -> DoctorReport:
    """Verify native clone support using the Rust probe."""
    record = invoke(operation=partial(_libcowtree.inspect_path, path=path))
    result = DOCTOR.validate_python(json.loads(record))
    return result
