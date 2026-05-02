from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
import json
from pathlib import Path

from inline_tests import test


class FilesystemKind(str, Enum):
    APFS = "apfs"
    REFLINK = "reflink"
    UNSUPPORTED = "unsupported"


class CloneTool(str, Enum):
    MACOS_CLONEFILE = "clonefile(2)"
    GNU_CP_REFLINK = "cp --reflink=always"


@dataclass(frozen=True)
class CommandResult:
    argv: list[str]
    returncode: int
    stdout: str = ""
    stderr: str = ""


@dataclass(frozen=True)
class WorktreeAddRequest:
    args: list[str] = field(default_factory=list)
    source: Path | None = None

    def git_args(self) -> list[str]:
        args = [arg for arg in self.args if arg != "--cow"]
        return args


@dataclass(frozen=True)
class Worktree:
    path: Path
    head: str | None = None
    branch: str | None = None
    detached: bool = False
    prunable: bool = False

    def to_json_text(self) -> str:
        text = json.dumps(
            {
                "path": str(self.path),
                "head": self.head,
                "branch": self.branch,
                "detached": self.detached,
                "prunable": self.prunable,
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
def strips_cow_marker_from_git_args() -> None:
    request = WorktreeAddRequest(args=["--cow", "-b", "scratch/x", "../x", "HEAD"])
    assert request.git_args() == ["-b", "scratch/x", "../x", "HEAD"]


@test
def reports_support_from_clone_tool() -> None:
    report = DoctorReport(path=Path("."), filesystem=FilesystemKind.APFS, clone_tool=CloneTool.MACOS_CLONEFILE)
    assert report.supported is True


@test
def serializes_worktree_paths_as_strings() -> None:
    worktree = Worktree(path=Path("/repo/wt"), head="abc", detached=True)
    assert worktree.to_json_text() == (
        '{"path": "/repo/wt", "head": "abc", "branch": null, "detached": true, "prunable": false}'
    )
