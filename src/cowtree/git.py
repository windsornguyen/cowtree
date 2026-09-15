from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
from pathlib import Path

from inline_tests import test

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.models import Worktree


def git_capture(runner: CommandRunner, args: list[str], *, cwd: Path | None = None, check: bool = True) -> str:
    result = runner.run(["git", *args], cwd=str(cwd) if cwd else None, check=check)
    stdout = result.stdout
    return stdout


def source_root(runner: CommandRunner, source: Path | None) -> Path:
    cwd = resolve_path(source) if source else None
    out = git_capture(runner, ["rev-parse", "--show-toplevel"], cwd=cwd)
    root = resolve_path(Path(out.removesuffix("\n")))
    return root


def resolve_path(path: Path) -> Path:
    try:
        resolved = path.resolve()
    except (RuntimeError, ValueError) as error:
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid path {path}: {error}") from error
    return resolved


def head(runner: CommandRunner, repo: Path) -> str:
    commit = git_capture(runner, ["-C", str(repo), "rev-parse", "HEAD"]).strip()
    return commit


def status_porcelain(runner: CommandRunner, repo: Path) -> str:
    status = git_capture(
        runner,
        [
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.ignorestat=false",
            "-c",
            "core.filemode=true",
            "-C",
            str(repo),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ],
    )
    return status


def config_bool(runner: CommandRunner, repo: Path, key: str) -> bool:
    result = runner.run(["git", "-C", str(repo), "config", "--bool", key], check=False)
    enabled = result.stdout.strip() == "true"
    return enabled


@contextmanager
def repository_lock(runner: CommandRunner, repo: Path) -> Iterator[None]:
    common = git_capture(runner, ["-C", str(repo), "rev-parse", "--path-format=absolute", "--git-common-dir"])
    # Never unlink this inode: waiters must all lock the same file.
    with (Path(common.removesuffix("\n")) / "cowtree.lock").open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


@dataclass(frozen=True)
class TrackedFile:
    mode: str
    path: str


def tracked_files(runner: CommandRunner, repo: Path, commit: str) -> list[TrackedFile]:
    data = git_capture(runner, ["-C", str(repo), "ls-tree", "-r", "-z", commit])
    files: list[TrackedFile] = []
    for record in data.split("\0"):
        if not record:
            continue
        metadata, path = record.split("\t", 1)
        mode = metadata.split()[0]
        if mode == "160000":
            raise CowtreeError(CowtreeErrorCode.SUBMODULE_UNSUPPORTED, f"submodules are unsupported: {path}")
        if mode not in ("100644", "100755", "120000"):
            raise CowtreeError(CowtreeErrorCode.UNSUPPORTED_MODE, f"unsupported tracked mode {mode}: {path}")
        files.append(TrackedFile(mode=mode, path=path))
    return files


def list_worktrees(runner: CommandRunner, repo: Path) -> list[Worktree]:
    data = git_capture(runner, ["-C", str(repo), "worktree", "list", "--porcelain", "-z"])
    worktrees = parse_worktree_porcelain(data)
    return worktrees


def parse_worktree_porcelain(data: str) -> list[Worktree]:
    records: list[list[str]] = []
    current: list[str] = []
    for field in data.split("\0"):
        if field == "":
            if current:
                records.append(current)
                current = []
            continue
        current.append(field)
    if current:
        records.append(current)

    worktrees: list[Worktree] = []
    for record in records:
        path: str | None = None
        head_value: str | None = None
        branch_value: str | None = None
        detached = False
        prunable = False
        locked = False
        reason: str | None = None
        for field in record:
            if field.startswith("worktree "):
                path = field.removeprefix("worktree ")
            elif field.startswith("HEAD "):
                head_value = field.removeprefix("HEAD ")
            elif field.startswith("branch "):
                branch_value = field.removeprefix("branch ")
            elif field == "detached":
                detached = True
            elif field.startswith("prunable"):
                prunable = True
            elif field == "locked" or field.startswith("locked "):
                locked = True
                reason = field.removeprefix("locked ") if field != "locked" else None
        if path is None:
            raise CowtreeError(CowtreeErrorCode.WORKTREE_NOT_FOUND, "git worktree list returned a record without path")
        worktrees.append(
            Worktree(
                path=Path(path),
                head=head_value,
                branch=branch_value,
                detached=detached,
                prunable=prunable,
                locked=locked,
                reason=reason,
            )
        )
    return worktrees


def ensure_clean_source(runner: CommandRunner, repo: Path) -> None:
    flags = git_capture(runner, ["-C", str(repo), "ls-files", "-v", "-z"])
    if any(record and (record[0].islower() or record[0] == "S") for record in flags.split("\0")):
        raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, "source index has assume-unchanged or skip-worktree entries")
    dirty = status_porcelain(runner, repo)
    if dirty:
        raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, "source has tracked changes")


def ensure_full_checkout(runner: CommandRunner, repo: Path) -> None:
    if config_bool(runner, repo, "core.sparseCheckout"):
        raise CowtreeError(CowtreeErrorCode.SPARSE_CHECKOUT, "sparse checkouts are not supported yet")


def ensure_same_head(runner: CommandRunner, source: Path, target: Path) -> None:
    if head(runner, source) != head(runner, target):
        raise CowtreeError(CowtreeErrorCode.HEAD_MISMATCH, "target HEAD differs from source HEAD")


@test
def parses_z_terminated_worktree_porcelain() -> None:
    data = "worktree /repo\0HEAD abc\0branch refs/heads/main\0\0worktree /repo/wt\0HEAD def\0detached\0\0"
    worktrees = parse_worktree_porcelain(data)
    assert worktrees[0].path == Path("/repo")
    assert worktrees[0].branch == "refs/heads/main"
    assert worktrees[1].detached is True


@test
def rejects_worktree_record_without_path() -> None:
    import pytest

    with pytest.raises(CowtreeError) as error:
        parse_worktree_porcelain("HEAD abc\0\0")
    assert error.value.code == CowtreeErrorCode.WORKTREE_NOT_FOUND
