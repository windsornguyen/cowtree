from __future__ import annotations

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
    cwd = source.resolve() if source else None
    out = git_capture(runner, ["rev-parse", "--show-toplevel"], cwd=cwd)
    root = Path(out.strip()).resolve()
    return root


def head(runner: CommandRunner, repo: Path) -> str:
    commit = git_capture(runner, ["-C", str(repo), "rev-parse", "HEAD"]).strip()
    return commit


def status_porcelain(runner: CommandRunner, repo: Path) -> str:
    status = git_capture(runner, ["-C", str(repo), "status", "--porcelain=v1", "--untracked-files=no"])
    return status


def config_bool(runner: CommandRunner, repo: Path, key: str) -> bool:
    result = runner.run(["git", "-C", str(repo), "config", "--bool", key], check=False)
    enabled = result.stdout.strip() == "true"
    return enabled


def worktree_paths(runner: CommandRunner, repo: Path) -> set[Path]:
    paths = {worktree.path for worktree in list_worktrees(runner, repo)}
    return paths


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
        values: dict[str, str | bool] = {}
        for field in record:
            if field.startswith("worktree "):
                values["path"] = field.removeprefix("worktree ")
            elif field.startswith("HEAD "):
                values["head"] = field.removeprefix("HEAD ")
            elif field.startswith("branch "):
                values["branch"] = field.removeprefix("branch ")
            elif field == "detached":
                values["detached"] = True
            elif field.startswith("prunable"):
                values["prunable"] = True
        path = values.get("path")
        if not isinstance(path, str):
            raise CowtreeError(CowtreeErrorCode.WORKTREE_NOT_FOUND, "git worktree list returned a record without path")
        head_value = values.get("head")
        branch_value = values.get("branch")
        worktrees.append(
            Worktree(
                path=Path(path),
                head=head_value if isinstance(head_value, str) else None,
                branch=branch_value if isinstance(branch_value, str) else None,
                detached=values.get("detached") is True,
                prunable=values.get("prunable") is True,
            )
        )
    return worktrees


def ensure_clean_source(runner: CommandRunner, repo: Path) -> None:
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
