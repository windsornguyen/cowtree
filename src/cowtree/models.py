from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
import json
from pathlib import Path

from inline_tests import test

from cowtree.errors import CowtreeError, CowtreeErrorCode


class FilesystemKind(str, Enum):
    APFS = "apfs"
    REFLINK = "reflink"
    UNSUPPORTED = "unsupported"


class CloneTool(str, Enum):
    MACOS_CLONEFILE = "clonefile(2)"
    LINUX_FICLONE = "FICLONE"


@dataclass(frozen=True)
class CommandResult:
    argv: list[str]
    returncode: int
    stdout: str = ""
    stderr: str = ""


@dataclass(frozen=True)
class WorktreeAddRequest:
    path: Path
    source: Path | None = None
    branch: str | None = None
    commitish: str = "HEAD"
    detach: bool = False
    lock: bool = False
    reason: str | None = None

    def __post_init__(self) -> None:
        for name, path in (("path", self.path), ("source", self.source)):
            if path is None and name == "source":
                continue
            if not isinstance(path, Path) or "\0" in str(path):
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"{name} must be a Path without NUL bytes")
        for name, value in (("branch", self.branch), ("commitish", self.commitish), ("reason", self.reason)):
            if value is None and name != "commitish":
                continue
            if not isinstance(value, str) or not value or "\0" in value:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, f"{name} must be a nonempty string without NUL bytes"
                )
        if not isinstance(self.detach, bool) or not isinstance(self.lock, bool):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "detach and lock must be booleans")
        if self.branch is not None and self.detach:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "branch and detach are mutually exclusive")
        if self.reason is not None and not self.lock:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "reason requires lock")


@dataclass(frozen=True)
class Worktree:
    path: Path
    head: str | None = None
    branch: str | None = None
    detached: bool = False
    prunable: bool = False
    locked: bool = False
    reason: str | None = None

    def to_json_text(self) -> str:
        text = json.dumps(
            {
                "path": str(self.path),
                "head": self.head,
                "branch": self.branch,
                "detached": self.detached,
                "prunable": self.prunable,
                "locked": self.locked,
                "reason": self.reason,
            }
        )
        return text


@dataclass(frozen=True)
class DoctorReport:
    path: Path
    filesystem: FilesystemKind
    clone_tool: CloneTool | None = None
    reason: str | None = None

    @property
    def supported(self) -> bool:
        supported = self.clone_tool is not None
        return supported


@test
def requires_only_the_destination_path() -> None:
    request = WorktreeAddRequest(path=Path("../x"))
    assert request.commitish == "HEAD"
    assert request.branch is None
    assert request.source is None
    assert request.detach is False
    assert request.lock is False
    assert request.reason is None


@test
def reports_support_from_clone_tool() -> None:
    report = DoctorReport(path=Path("."), filesystem=FilesystemKind.APFS, clone_tool=CloneTool.MACOS_CLONEFILE)
    assert report.supported is True


@test
def serializes_worktree_paths_as_strings() -> None:
    worktree = Worktree(path=Path("/repo/wt"), head="abc", detached=True)
    assert worktree.to_json_text() == (
        '{"path": "/repo/wt", "head": "abc", "branch": null, "detached": true, '
        '"prunable": false, "locked": false, "reason": null}'
    )
