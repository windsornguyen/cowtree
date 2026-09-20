"""Typed command inputs, Git checkout records, and filesystem reports."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from enum import Enum
import json
from pathlib import Path

from inline_tests import test

from cowtree.errors import CowtreeError, CowtreeErrorCode


# --- Command inputs ---


class SourceMode(str, Enum):
    """Choose working-checkout bytes or an explicitly materialized Git commit."""

    CHECKOUT = "checkout"
    COMMIT = "commit"


class Command(str, Enum):
    """Supported command-line operations."""

    ADD = "add"
    LIST = "list"
    REMOVE = "remove"
    DOCTOR = "doctor"
    HELP = "help"


@dataclass
class Arguments(argparse.Namespace):
    """Arguments populated by the command-line parser."""

    command: Command = Command.HELP
    path: Path | None = None
    branch: str | None = None
    existing_branch: str | None = None
    commitish: str = "HEAD"
    detach: bool = False
    lock: bool = False
    reason: str | None = None
    json: bool = False
    force: bool = False
    source_mode: SourceMode = SourceMode.CHECKOUT


# --- Filesystem records ---


class FilesystemKind(str, Enum):
    """Filesystem families supported by a successful clone probe."""

    APFS = "apfs"
    REFLINK = "reflink"
    UNSUPPORTED = "unsupported"


class CloneTool(str, Enum):
    """Native primitives used to clone regular-file storage."""

    MACOS_CLONEFILE = "clonefile(2)"
    LINUX_FICLONE = "FICLONE"


@dataclass(frozen=True)
class CommandResult:
    """Captured output and exit status from one process."""

    argv: list[str]
    returncode: int
    stdout: str = ""
    stderr: str = ""


# --- Git records ---


class FileMode(str, Enum):
    """Tracked file modes that can be cloned into a worktree."""

    REGULAR = "100644"
    EXECUTABLE = "100755"
    SYMLINK = "120000"


@dataclass(frozen=True)
class TrackedFile:
    """File mode and repository-relative path recorded in a Git tree."""

    mode: FileMode
    path: str


@dataclass(frozen=True)
class Checkout:
    """Commit and tracked files captured from a clean source checkout."""

    commit: str
    files: tuple[TrackedFile, ...]


@dataclass(frozen=True)
class WorktreeAddRequest:
    """Require a destination and a consistent branch, checkout, and lock policy."""

    path: Path
    source: Path | None = None
    branch: str | None = None
    commitish: str = "HEAD"
    detach: bool = False
    lock: bool = False
    reason: str | None = None
    existing_branch: str | None = None
    source_mode: SourceMode = SourceMode.CHECKOUT

    def __post_init__(self) -> None:
        """Reject invalid input fields before starting any filesystem operation."""
        if not isinstance(self.source_mode, SourceMode):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "source_mode must be a SourceMode"
            )
        for name, path in (("path", self.path), ("source", self.source)):
            if path is None and name == "source":
                continue
            if not isinstance(path, Path):
                raise CowtreeError(
                    code=CowtreeErrorCode.INVALID_ARGUMENTS, message=f"{name} must be a Path"
                )
            if "\0" in str(path):
                raise CowtreeError(
                    code=CowtreeErrorCode.INVALID_ARGUMENTS,
                    message=f"{name} must not contain NUL bytes",
                )
        for name, text in (
            ("branch", self.branch),
            ("existing_branch", self.existing_branch),
            ("commitish", self.commitish),
            ("reason", self.reason),
        ):
            if text is None and name != "commitish":
                continue
            if not isinstance(text, str):
                raise CowtreeError(
                    code=CowtreeErrorCode.INVALID_ARGUMENTS, message=f"{name} must be a string"
                )
            if not text:
                raise CowtreeError(
                    code=CowtreeErrorCode.INVALID_ARGUMENTS, message=f"{name} must not be empty"
                )
            if "\0" in text:
                raise CowtreeError(
                    code=CowtreeErrorCode.INVALID_ARGUMENTS,
                    message=f"{name} must not contain NUL bytes",
                )
        for name, flag in (("detach", self.detach), ("lock", self.lock)):
            if not isinstance(flag, bool):
                raise CowtreeError(
                    code=CowtreeErrorCode.INVALID_ARGUMENTS, message=f"{name} must be a boolean"
                )
        self.validate_policy()

    def validate_policy(self) -> None:
        """Reject contradictory branch and lock policies."""
        if self.branch is not None and self.detach:
            raise CowtreeError(
                code=CowtreeErrorCode.INVALID_ARGUMENTS,
                message="branch and detach are mutually exclusive",
            )
        if self.existing_branch is not None and self.branch is not None:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "existing_branch excludes branch"
            )
        if self.existing_branch is not None and self.detach:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "existing_branch excludes detach"
            )
        if self.reason is not None and not self.lock:
            raise CowtreeError(
                code=CowtreeErrorCode.INVALID_ARGUMENTS, message="reason requires lock"
            )


@dataclass(frozen=True)
class Worktree:
    """Worktree registration returned by Git, including lock and branch metadata."""

    path: Path
    head: str | None = None
    branch: str | None = None
    detached: bool = False
    prunable: bool = False
    locked: bool = False
    reason: str | None = None

    def to_json_text(self) -> str:
        """Serialize metadata with filesystem paths represented as strings."""
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
    """Native clone probe result for an existing directory."""

    path: Path
    filesystem: FilesystemKind
    clone_tool: CloneTool | None = None
    reason: str | None = None

    @property
    def supported(self) -> bool:
        """Report whether the native clone probe succeeded."""
        supported = self.clone_tool is not None
        return supported


# --- Tests ---


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
    report = DoctorReport(
        path=Path("."), filesystem=FilesystemKind.APFS, clone_tool=CloneTool.MACOS_CLONEFILE
    )
    assert report.supported is True


@test
def serializes_worktree_paths_as_strings() -> None:
    worktree = Worktree(path=Path("/repo/wt"), head="abc", detached=True)
    assert worktree.to_json_text() == (
        '{"path": "/repo/wt", "head": "abc", "branch": null, "detached": true, '
        '"prunable": false, "locked": false, "reason": null}'
    )
