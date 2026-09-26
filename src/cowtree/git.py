"""Repository queries, checkout validation, and cooperative worktree locking."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
import sys

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.locks import exclusive
from cowtree.submodule_types import PinnedSubmodule, SubmodulePolicy
from cowtree.types import Checkout, CommandResult, FileMode, TrackedFile, Worktree


@dataclass(frozen=True)
class GitRepository:
    """Own Git commands and the common lock for one checkout."""

    io: CommandRunner
    path: Path

    @classmethod
    def discover(cls, io: CommandRunner, source: Path | None = None) -> GitRepository:
        """Resolve the checkout containing source or the caller's current directory."""
        cwd = None if source is None else str(resolve_path(path=source))
        result = io.run(argv=["git", "rev-parse", "--show-toplevel"], cwd=cwd)
        root = resolve_path(path=Path(result.stdout.removesuffix("\n")))
        repository = cls(io=io, path=root)
        return repository

    def run(self, args: list[str], *, check: bool = True) -> CommandResult:
        """Run Git against this checkout with explicit argument boundaries."""
        result = self.io.run(argv=["git", "-C", str(self.path), *args], check=check)
        return result

    def capture(self, args: list[str]) -> str:
        """Read Git output without altering filename bytes or line endings."""
        result = self.run(args=args)
        stdout = result.stdout
        return stdout

    def head(self) -> str:
        """Read the checkout's current commit."""
        commit = self.capture(args=["rev-parse", "HEAD"]).removesuffix("\n")
        return commit

    def status(self) -> str:
        """Read tracked changes without trusting fsmonitor or hidden mode differences."""
        status = self.capture(
            args=[
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.ignorestat=false",
                "-c",
                "core.filemode=false" if sys.platform == "win32" else "core.filemode=true",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=no",
                "--ignore-submodules=none",
            ]
        )
        return status

    def check_detached(self, expected: tuple[str, ...]) -> None:
        """Refuse to move a caller's branch or an unexpected managed HEAD."""
        result = self.run(args=["symbolic-ref", "--quiet", "HEAD"], check=False)
        if result.returncode == 0:
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "managed leaf must remain detached"
            )
        if result.returncode != 1:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, result.stderr)
        if self.head() not in expected:
            raise CowtreeError(CowtreeErrorCode.HEAD_MISMATCH, "managed leaf HEAD changed")

    @contextmanager
    def lock(self) -> Iterator[int]:
        """Serialize complete cowtree operations across all linked worktrees."""
        common = self.capture(args=["rev-parse", "--path-format=absolute", "--git-common-dir"])
        path = Path(common.removesuffix("\n")) / "cowtree.lock"
        # Keep one inode for all waiters.
        with path.open("a+b") as lock, exclusive(descriptor=lock.fileno()):
            yield lock.fileno()

    def snapshot(self, *, submodules: SubmodulePolicy = SubmodulePolicy.REJECT) -> Checkout:
        """Pin a complete, clean source checkout and its tracked manifest."""
        configured = self.run(args=["config", "--bool", "core.sparseCheckout"], check=False)
        if configured.returncode not in (0, 1):
            raise CowtreeError(code=CowtreeErrorCode.COMMAND_FAILED, message=configured.stderr)
        if configured.stdout.removesuffix("\n") == "true":
            raise CowtreeError(
                code=CowtreeErrorCode.SPARSE_CHECKOUT, message="sparse checkouts are unsupported"
            )
        commit = self.head()
        data = self.capture(args=["ls-tree", "-r", "-z", commit])
        tree = parse_tree(data=data, submodules=submodules)
        if submodules is SubmodulePolicy.LEAVE_UNINITIALIZED:
            self.require_uninitialized(pins=tree.submodules)
        flags = self.capture(args=["ls-files", "-v", "-z"])
        for record in flags.split("\0"):
            if not record:
                continue
            if record[0].islower():
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, "source has assume-unchanged entries"
                )
            if record[0] == "S":
                raise CowtreeError(
                    CowtreeErrorCode.DIRTY_SOURCE, "source has skip-worktree entries"
                )
        if self.status():
            raise CowtreeError(
                code=CowtreeErrorCode.DIRTY_SOURCE, message="source has tracked changes"
            )
        checkout = Checkout(commit=commit, files=tree.files, submodules=tree.submodules)
        return checkout

    def require_uninitialized(self, pins: tuple[PinnedSubmodule, ...]) -> None:
        """Refuse a gitlink whose source path holds anything, since none of it would be copied."""
        for pin in pins:
            path = self.path / pin.path
            if path.is_symlink() or (path.exists() and not path.is_dir()):
                raise CowtreeError(
                    CowtreeErrorCode.SUBMODULE_INITIALIZED,
                    f"submodule path is not a directory: {pin.path}",
                )
            if path.is_dir() and any(path.iterdir()):
                raise CowtreeError(
                    CowtreeErrorCode.SUBMODULE_INITIALIZED, f"submodule is initialized: {pin.path}"
                )

    def worktrees(self) -> list[Worktree]:
        """Read every worktree registration in the repository."""
        data = self.capture(args=["worktree", "list", "--porcelain", "-z"])
        worktrees = parse_worktree_porcelain(data=data)
        return worktrees

    def has_branch(self, branch: str) -> bool:
        """Check a literal local branch, preserving unexpected Git failures."""
        result = self.run(
            args=["show-ref", "--verify", "--quiet", f"refs/heads/{branch}"], check=False
        )
        if result.returncode not in (0, 1):
            raise CowtreeError(code=CowtreeErrorCode.COMMAND_FAILED, message=result.stderr)
        exists = result.returncode == 0
        return exists

    def find_worktree(self, path: Path) -> Worktree | None:
        """Match registrations by filesystem identity, including normalized names."""
        for worktree in self.worktrees():
            if worktree.path == path:
                return worktree
            try:
                matches = worktree.path.samefile(path)
            except FileNotFoundError:
                # Git also lists missing, prunable worktrees.
                continue
            if matches:
                return worktree
        return None


def resolve_path(path: Path) -> Path:
    """Resolve a caller path, preserving invalid-path errors at the API boundary."""
    try:
        resolved = path.resolve()
    except (RuntimeError, ValueError) as error:
        raise CowtreeError(
            CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid path {path}: {error}"
        ) from error
    return resolved


def parse_tracked_files(
    data: str, submodules: SubmodulePolicy = SubmodulePolicy.REJECT
) -> tuple[TrackedFile, ...]:
    """Decode a Git tree into supported file modes and byte-preserving paths."""
    files = parse_tree(data=data, submodules=submodules).files
    return files


@dataclass(frozen=True)
class GitTree:
    files: tuple[TrackedFile, ...]
    submodules: tuple[PinnedSubmodule, ...]


def parse_tree(data: str, submodules: SubmodulePolicy) -> GitTree:
    """Preserve gitlinks only under a policy that admits them."""
    if not isinstance(submodules, SubmodulePolicy):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid submodule policy")
    files: list[TrackedFile] = []
    pins: list[PinnedSubmodule] = []
    for record in data.split("\0"):
        if not record:
            continue
        metadata, path = record.split("\t", 1)
        mode, _, commit = metadata.split()
        if mode == "160000":
            match submodules:
                case SubmodulePolicy.REJECT:
                    raise CowtreeError(
                        CowtreeErrorCode.SUBMODULE_UNSUPPORTED,
                        f"submodules are unsupported: {path}",
                    )
                case SubmodulePolicy.LEAVE_UNINITIALIZED | SubmodulePolicy.MATERIALIZE_PINNED:
                    pass
                case _:
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS, "invalid submodule policy"
                    )
            pins.append(PinnedSubmodule(path=path, commit=commit))
            continue
        try:
            file_mode = FileMode(mode)
        except ValueError as error:
            raise CowtreeError(
                code=CowtreeErrorCode.UNSUPPORTED_MODE,
                message=f"unsupported tracked mode {mode}: {path}",
            ) from error
        files.append(TrackedFile(mode=file_mode, path=path))
    manifest = GitTree(files=tuple(files), submodules=tuple(pins))
    return manifest


def parse_worktree_porcelain(data: str) -> list[Worktree]:
    """Decode NUL-delimited Git worktree records."""
    worktrees = [
        parse_worktree_record(fields=record.split("\0")) for record in data.split("\0\0") if record
    ]
    return worktrees


def parse_worktree_record(fields: list[str]) -> Worktree:
    """Decode one registration and require its pathname."""
    path: str | None = None
    head: str | None = None
    branch: str | None = None
    detached = False
    prunable = False
    locked = False
    reason: str | None = None
    for field in fields:
        key, _, value = field.partition(" ")
        match key:
            case "worktree":
                path = value
            case "HEAD":
                head = value
            case "branch":
                branch = value
            case "detached":
                detached = True
            case "prunable":
                prunable = True
            case "locked":
                locked = True
                reason = value if value else None
    if path is None:
        raise CowtreeError(
            code=CowtreeErrorCode.WORKTREE_NOT_FOUND, message="worktree record has no path"
        )
    worktree = Worktree(
        path=Path(path),
        head=head,
        branch=branch,
        detached=detached,
        prunable=prunable,
        locked=locked,
        reason=reason,
    )
    return worktree


# --- Tests ---

from inline_tests import test  # noqa: E402


@test
def parses_z_terminated_worktree_porcelain() -> None:
    data = (
        "worktree /repo\0HEAD abc\0branch refs/heads/main\0\0"
        "worktree /repo/wt\0HEAD def\0detached\0\0"
    )
    worktrees = parse_worktree_porcelain(data=data)
    assert worktrees[0].path == Path("/repo")
    assert worktrees[0].branch == "refs/heads/main"
    assert worktrees[1].detached is True


@test
def rejects_worktree_record_without_path() -> None:
    import pytest

    with pytest.raises(CowtreeError) as error:
        parse_worktree_porcelain(data="HEAD abc\0\0")
    assert error.value.code == CowtreeErrorCode.WORKTREE_NOT_FOUND
