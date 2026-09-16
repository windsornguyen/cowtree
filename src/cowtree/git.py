"""Repository queries, checkout validation, and cooperative worktree locking."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
from pathlib import Path

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
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
                "core.filemode=true",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=no",
                "--ignore-submodules=none",
            ]
        )
        return status

    @contextmanager
    def lock(self) -> Iterator[None]:
        """Serialize complete cowtree operations across all linked worktrees."""
        common = self.capture(args=["rev-parse", "--path-format=absolute", "--git-common-dir"])
        path = Path(common.removesuffix("\n")) / "cowtree.lock"
        # Keep one inode for all waiters.
        with path.open("a+b") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)

    def snapshot(self) -> Checkout:
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
        files = parse_tracked_files(data=data)
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
        checkout = Checkout(commit=commit, files=files)
        return checkout

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


def parse_tracked_files(data: str) -> tuple[TrackedFile, ...]:
    """Decode a Git tree into supported file modes and byte-preserving paths."""
    files: list[TrackedFile] = []
    for record in data.split("\0"):
        if not record:
            continue
        metadata, path = record.split("\t", 1)
        mode = metadata.split()[0]
        if mode == "160000":
            raise CowtreeError(
                CowtreeErrorCode.SUBMODULE_UNSUPPORTED, f"submodules are unsupported: {path}"
            )
        try:
            file_mode = FileMode(mode)
        except ValueError as error:
            raise CowtreeError(
                code=CowtreeErrorCode.UNSUPPORTED_MODE,
                message=f"unsupported tracked mode {mode}: {path}",
            ) from error
        files.append(TrackedFile(mode=file_mode, path=path))
    manifest = tuple(files)
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
