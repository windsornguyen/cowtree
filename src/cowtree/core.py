from __future__ import annotations

import os
from pathlib import Path

from inline_tests import test

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.fs import clone_regular_file, doctor, ensure_cow_supported
from cowtree.git import (
    ensure_clean_source,
    ensure_full_checkout,
    ensure_same_head,
    git_capture,
    list_worktrees,
    source_root,
    status_porcelain,
    worktree_paths,
)
from cowtree.models import DoctorReport, Worktree, WorktreeAddRequest


def add_worktree(request: WorktreeAddRequest, runner: CommandRunner | None = None) -> Worktree:
    runner = runner or CommandRunner()
    source = source_root(runner, request.source)
    args = request.git_args()
    if "--checkout" in args:
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "--checkout conflicts with CoW checkout")

    ensure_clean_source(runner, source)
    ensure_full_checkout(runner, source)

    before = worktree_paths(runner, source)
    runner.run(["git", "-C", str(source), "worktree", "add", "--no-checkout", *args])
    target = find_added_worktree(source, before, runner)

    try:
        ensure_same_head(runner, source, target)
        ensure_cow_supported(source, target, runner)
        copy_tracked_files(source, target, runner)
        runner.run(["git", "-C", str(target), "reset", "--mixed", "-q", "HEAD"])
        dirty = status_porcelain(runner, target)
        if dirty:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"target is dirty after CoW checkout:\n{dirty}")
    except BaseException:
        runner.run(["git", "-C", str(source), "worktree", "remove", "--force", str(target)], check=False)
        raise

    worktree = next(worktree for worktree in list_worktrees(runner, source) if worktree.path == target)
    return worktree


def list_all_worktrees(source: Path | None = None, runner: CommandRunner | None = None) -> list[Worktree]:
    runner = runner or CommandRunner()
    worktrees = list_worktrees(runner, source_root(runner, source))
    return worktrees


def remove_worktree(
    path: Path,
    *,
    source: Path | None = None,
    force: bool = False,
    runner: CommandRunner | None = None,
) -> None:
    runner = runner or CommandRunner()
    repo = source_root(runner, source)
    args = ["git", "-C", str(repo), "worktree", "remove"]
    if force:
        args.append("--force")
    args.append(str(path))
    runner.run(args)


def inspect_path(path: Path, runner: CommandRunner | None = None) -> DoctorReport:
    report = doctor(path, runner)
    return report


def find_added_worktree(source: Path, before: set[Path], runner: CommandRunner) -> Path:
    after = worktree_paths(runner, source)
    added = after - before
    if len(added) != 1:
        raise CowtreeError(CowtreeErrorCode.WORKTREE_NOT_FOUND, f"expected one new worktree, found {len(added)}")
    target = next(iter(added)).resolve()
    return target


def copy_tracked_files(source: Path, target: Path, runner: CommandRunner) -> None:
    records = git_capture(runner, ["-C", str(source), "ls-files", "-s", "-z"]).split("\0")
    for record in records:
        if not record:
            continue
        meta, path = record.split("\t", 1)
        mode = meta.split()[0]
        src = source / path
        dst = target / path
        dst.parent.mkdir(parents=True, exist_ok=True)
        if mode in ("100644", "100755"):
            clone_regular_file(src, dst, runner)
        elif mode == "120000":
            os.symlink(os.readlink(src), dst)
        elif mode == "160000":
            raise CowtreeError(CowtreeErrorCode.SUBMODULE_UNSUPPORTED, f"submodules are not supported yet: {path}")
        else:
            raise CowtreeError(CowtreeErrorCode.UNSUPPORTED_MODE, f"unsupported git file mode {mode} for {path}")


@test
def creates_clean_cow_worktree(tmp_path: Path) -> None:
    import pytest

    runner = CommandRunner()
    source = tmp_path / "repo"
    target = tmp_path / "cow"
    source.mkdir()
    runner.run(["git", "init", "-q", str(source)])
    runner.run(["git", "-C", str(source), "config", "user.email", "test@example.com"])
    runner.run(["git", "-C", str(source), "config", "user.name", "Test"])
    (source / "a.txt").write_text("one\n")
    (source / "run.sh").write_text("#!/bin/sh\necho hi\n")
    (source / "run.sh").chmod(0o755)
    os.symlink("a.txt", source / "link-a")
    runner.run(["git", "-C", str(source), "add", "."])
    runner.run(["git", "-C", str(source), "commit", "-qm", "init"])

    report = doctor(target.parent, runner)
    if not report.supported:
        pytest.skip(report.reason or "CoW unavailable")

    worktree = add_worktree(
        WorktreeAddRequest(source=source, args=["-b", "cow-branch", str(target), "HEAD"]),
        runner,
    )
    assert worktree.path == target.resolve()
    assert status_porcelain(runner, target) == ""
    assert os.readlink(target / "link-a") == "a.txt"
    assert os.access(target / "run.sh", os.X_OK)


@test
def cleans_up_failed_head_mismatch(tmp_path: Path) -> None:
    import pytest

    runner = CommandRunner()
    source = tmp_path / "repo"
    target = tmp_path / "old"
    source.mkdir()
    runner.run(["git", "init", "-q", str(source)])
    runner.run(["git", "-C", str(source), "config", "user.email", "test@example.com"])
    runner.run(["git", "-C", str(source), "config", "user.name", "Test"])
    (source / "a.txt").write_text("one\n")
    runner.run(["git", "-C", str(source), "add", "."])
    runner.run(["git", "-C", str(source), "commit", "-qm", "first"])
    (source / "a.txt").write_text("two\n")
    runner.run(["git", "-C", str(source), "commit", "-am", "second", "-q"])

    with pytest.raises(CowtreeError) as error:
        add_worktree(WorktreeAddRequest(source=source, args=["-b", "old-branch", str(target), "HEAD~1"]), runner)
    assert error.value.code == CowtreeErrorCode.HEAD_MISMATCH

    assert not target.exists()
