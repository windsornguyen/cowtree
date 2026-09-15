"""Create, list, remove, and inspect CoW worktrees under the README contract."""

from __future__ import annotations

from dataclasses import dataclass, field
import os
from pathlib import Path
import stat

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.fs import doctor
from cowtree.git import GitRepository, resolve_path
from cowtree.native import clone_regular_file
from cowtree.types import (
    Checkout,
    DoctorReport,
    FileMode,
    TrackedFile,
    Worktree,
    WorktreeAddRequest,
)


# --- Public operations ---


def add_worktree(request: WorktreeAddRequest, runner: CommandRunner | None = None) -> Worktree:
    """Clone a clean source at its current commit and create the requested branch."""
    if runner is None:
        runner = CommandRunner()
    try:
        repository = GitRepository.discover(io=runner, source=request.source)
        with repository.lock():
            creation = WorktreeCreation.prepare(repository=repository, request=request)
            worktree = creation.run()
    except OSError as error:
        raise CowtreeError(
            code=CowtreeErrorCode.COMMAND_FAILED, message=f"add failed: {error}"
        ) from error
    return worktree


def list_all_worktrees(
    source: Path | None = None, runner: CommandRunner | None = None
) -> list[Worktree]:
    """Return every Git worktree, including checkouts created outside cowtree."""
    if runner is None:
        runner = CommandRunner()
    try:
        repository = GitRepository.discover(io=runner, source=source)
        with repository.lock():
            worktrees = repository.worktrees()
    except OSError as error:
        raise CowtreeError(
            code=CowtreeErrorCode.COMMAND_FAILED, message=f"list failed: {error}"
        ) from error
    return worktrees


def remove_worktree(
    path: Path,
    *,
    source: Path | None = None,
    force: bool = False,
    runner: CommandRunner | None = None,
) -> None:
    """Remove a linked worktree using Git's dirty and lock checks. Preserve its branch."""
    if runner is None:
        runner = CommandRunner()
    try:
        target = resolve_path(path=path)
        repository = GitRepository.discover(io=runner, source=source)
        with repository.lock():
            args = ["worktree", "remove"]
            if force:
                args.append("--force")
            repository.run(args=[*args, "--", str(target)])
    except OSError as error:
        raise CowtreeError(
            code=CowtreeErrorCode.COMMAND_FAILED, message=f"remove failed: {error}"
        ) from error


def inspect_path(path: Path, runner: CommandRunner | None = None) -> DoctorReport:
    """Probe native CoW support in an existing directory without requiring Git."""
    report = doctor(path=path, runner=runner)
    return report


@dataclass
class WorktreeCreation:
    """Own one pinned checkout and only the directories created by its transaction."""

    repository: GitRepository
    request: WorktreeAddRequest
    checkout: Checkout
    target: Path
    created: list[Path] = field(default_factory=list, init=False)

    @classmethod
    def prepare(cls, repository: GitRepository, request: WorktreeAddRequest) -> WorktreeCreation:
        """Resolve the destination and pin a valid source before acquiring ownership."""
        target = request.path.absolute()
        if os.path.lexists(target):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, f"destination already exists: {target}"
            )
        target = resolve_path(path=target)
        checkout = repository.snapshot()
        requested = repository.capture(
            args=[
                "rev-parse",
                "--verify",
                "--end-of-options",
                f"{request.commitish}^{{commit}}",
            ]
        ).removesuffix("\n")
        if requested != checkout.commit:
            raise CowtreeError(
                CowtreeErrorCode.HEAD_MISMATCH, "requested commit differs from source HEAD"
            )
        branch = request.branch
        if branch is not None:
            checked = repository.capture(args=["check-ref-format", "--branch", branch])
            if checked.removesuffix("\n") != branch:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "branch must be a literal name"
                )
            if repository.has_branch(branch=branch):
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, f"branch already exists: {branch}"
                )
        creation = cls(repository=repository, request=request, checkout=checkout, target=target)
        return creation

    def run(self) -> Worktree:
        """Register and populate the worktree. Roll back all owned state on exceptions."""
        try:
            self.reserve()
            self.register()
            self.copy_files()
            self.checkout_index()
            if not self.request.lock:
                self.repository.run(args=["worktree", "unlock", str(self.target)])
            worktree = self.repository.find_worktree(path=self.target)
            if worktree is None:
                raise CowtreeError(
                    CowtreeErrorCode.WORKTREE_NOT_FOUND,
                    f"created worktree is not registered: {self.target}",
                )
        except BaseException as original:
            # Roll back cancellation, then re-raise it.
            try:
                self.rollback()
            except (CowtreeError, OSError) as cleanup:
                raise CowtreeError(
                    code=CowtreeErrorCode.CLEANUP_FAILED,
                    message=f"{original}; cleanup failed for {self.target}, "
                    f"branch {self.request.branch!r}: "
                    f"{cleanup}; inspect git worktree list and refs",
                ) from original
            raise
        return worktree

    def reserve(self) -> None:
        """Check clone support before exclusively creating the target and missing parents."""
        missing: list[Path] = []
        parent = self.target.parent
        while not parent.exists():
            missing.append(parent)
            parent = parent.parent
        if self.repository.path.stat().st_dev != parent.stat().st_dev:
            raise CowtreeError(
                CowtreeErrorCode.DIFFERENT_FILESYSTEM, "source and target filesystems differ"
            )
        report = doctor(path=parent, runner=self.repository.io)
        if not report.supported:
            raise CowtreeError(
                CowtreeErrorCode.COW_UNAVAILABLE, f"CoW unavailable: {report.reason}"
            )
        for directory in [*reversed(missing), self.target]:
            directory.mkdir()
            self.created.append(directory)

    def register(self) -> None:
        """Create the locked Git registration at the pinned commit."""
        args = ["worktree", "add", "--no-checkout", "--lock"]
        if self.request.reason is not None:
            args.extend(["--reason", self.request.reason])
        branch = self.request.branch
        args.extend(["-b", branch] if branch is not None else ["--detach"])
        self.repository.run(args=[*args, "--", str(self.target), self.checkout.commit])

    def copy_files(self) -> None:
        """Populate only paths recorded in the pinned source tree."""
        for entry in self.checkout.files:
            self.copy_file(entry=entry)

    def copy_file(self, entry: TrackedFile) -> None:
        """Clone one tracked file or preserve its symlink text."""
        source = self.repository.path / entry.path
        target = self.target / entry.path
        mode = source.lstat().st_mode
        target.parent.mkdir(parents=True, exist_ok=True)
        if entry.mode is FileMode.SYMLINK:
            if not stat.S_ISLNK(mode):
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, f"tracked file type changed: {entry.path}"
                )
            os.symlink(os.readlink(source), target)
            return
        if not stat.S_ISREG(mode):
            raise CowtreeError(
                CowtreeErrorCode.DIRTY_SOURCE, f"tracked file type changed: {entry.path}"
            )
        if bool(mode & stat.S_IXUSR) != (entry.mode is FileMode.EXECUTABLE):
            raise CowtreeError(
                CowtreeErrorCode.DIRTY_SOURCE, f"tracked executable mode changed: {entry.path}"
            )
        clone_regular_file(source=source, target=target)

    def checkout_index(self) -> None:
        """Build the index and reject changed commits or mismatched target bytes."""
        target = GitRepository(io=self.repository.io, path=self.target)
        target.run(
            args=[
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.ignorestat=false",
                "reset",
                "--mixed",
                "-q",
                self.checkout.commit,
            ]
        )
        if self.repository.head() != self.checkout.commit:
            raise CowtreeError(
                CowtreeErrorCode.HEAD_MISMATCH, "source HEAD changed during CoW checkout"
            )
        if target.head() != self.checkout.commit:
            raise CowtreeError(
                CowtreeErrorCode.HEAD_MISMATCH, "target HEAD changed during CoW checkout"
            )
        if target.status():
            raise CowtreeError(
                code=CowtreeErrorCode.DIRTY_SOURCE, message="source changed during CoW checkout"
            )

    def rollback(self) -> None:
        """Remove owned registration, branch, and directories in that order."""
        if self.target in self.created:
            registered = self.repository.find_worktree(path=self.target) is not None
            if registered:
                self.repository.run(
                    args=["worktree", "remove", "--force", "--force", str(self.target)]
                )
            elif self.target.exists():
                self.target.rmdir()
            self.created.remove(self.target)
            branch = self.request.branch
            if branch is not None and self.repository.has_branch(branch=branch):
                # Preserve refs moved by another writer.
                self.repository.run(
                    args=["update-ref", "-d", f"refs/heads/{branch}", self.checkout.commit]
                )
        for directory in reversed(self.created):
            directory.rmdir()
