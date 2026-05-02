from __future__ import annotations

from enum import Enum
from pathlib import Path

from inline_tests import test
from pydantic import BaseModel, ConfigDict, Field


class FilesystemKind(str, Enum):
    APFS = "apfs"
    REFLINK = "reflink"
    UNSUPPORTED = "unsupported"


class CloneTool(str, Enum):
    MACOS_CLONEFILE = "clonefile(2)"
    GNU_CP_REFLINK = "cp --reflink=always"


class CommandResult(BaseModel):
    model_config = ConfigDict(frozen=True)

    argv: list[str] = Field(min_length=1)
    returncode: int
    stdout: str = ""
    stderr: str = ""


class WorktreeAddRequest(BaseModel):
    model_config = ConfigDict(frozen=True, arbitrary_types_allowed=True)

    args: list[str] = Field(default_factory=list)
    source: Path | None = None

    def git_args(self) -> list[str]:
        args = [arg for arg in self.args if arg != "--cow"]
        return args


class Worktree(BaseModel):
    model_config = ConfigDict(frozen=True, arbitrary_types_allowed=True)

    path: Path
    head: str | None = None
    branch: str | None = None
    detached: bool = False
    prunable: bool = False


class DoctorReport(BaseModel):
    model_config = ConfigDict(frozen=True, arbitrary_types_allowed=True)

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
