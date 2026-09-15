"""Create, enumerate, remove, and inspect CoW worktrees under the README contract."""

from __future__ import annotations

import os
from pathlib import Path
import stat

from inline_tests import test

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.fs import clone_regular_file, doctor, ensure_cow_supported
from cowtree.git import (
    TrackedFile,
    ensure_clean_source,
    ensure_full_checkout,
    git_capture,
    head,
    list_worktrees,
    repository_lock,
    resolve_path,
    source_root,
    status_porcelain,
    tracked_files,
)
from cowtree.models import DoctorReport, Worktree, WorktreeAddRequest


def add_worktree(request: WorktreeAddRequest, runner: CommandRunner | None = None) -> Worktree:
    """Clone a clean source at its current commit; create only the requested branch."""
    runner = runner or CommandRunner()
    try:
        source = source_root(runner, request.source)
        with repository_lock(runner, source):
            worktree = add_locked(request, source, runner)
    except OSError as error:
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"add failed: {error}") from error
    return worktree


def add_locked(request: WorktreeAddRequest, source: Path, runner: CommandRunner) -> Worktree:
    target = request.path.absolute()
    if os.path.lexists(target):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"destination already exists: {target}")
    target = resolve_path(target)
    ensure_full_checkout(runner, source)
    commit = head(runner, source)
    files = tracked_files(runner, source, commit)
    ensure_clean_source(runner, source)
    requested = git_capture(
        runner,
        [
            "-C",
            str(source),
            "rev-parse",
            "--verify",
            "--end-of-options",
            f"{request.commitish}^{{commit}}",
        ],
    ).removesuffix("\n")
    if requested != commit:
        raise CowtreeError(CowtreeErrorCode.HEAD_MISMATCH, "requested commit differs from source HEAD")
    validate_branch(request.branch, source, runner)
    parent = target.parent
    missing_parents: list[Path] = []
    while not parent.exists():
        missing_parents.append(parent)
        parent = parent.parent
    ensure_cow_supported(source, parent, runner)
    created_parents: list[Path] = []
    target_owned = False
    try:
        for directory in reversed(missing_parents):
            directory.mkdir()
            created_parents.append(directory)
        target.mkdir()
        target_owned = True
        args = ["git", "-C", str(source), "worktree", "add", "--no-checkout", "--lock"]
        if request.reason is not None:
            args.extend(["--reason", request.reason])
        args.extend(["-b", request.branch] if request.branch is not None else ["--detach"])
        runner.run([*args, "--", str(target), commit])
        copy_tracked_files(source, target, files)
        runner.run(
            [
                "git",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.ignorestat=false",
                "-C",
                str(target),
                "reset",
                "--mixed",
                "-q",
                commit,
            ]
        )
        if head(runner, source) != commit or head(runner, target) != commit:
            raise CowtreeError(CowtreeErrorCode.HEAD_MISMATCH, "HEAD changed during CoW checkout")
        if status_porcelain(runner, target):
            raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, "source changed during CoW checkout")
        if not request.lock:
            runner.run(["git", "-C", str(source), "worktree", "unlock", str(target)])
        worktree = next(tree for tree in list_worktrees(runner, source) if tree.path == target)
    except BaseException as original:
        if target_owned:
            rollback_add(target, request.branch, commit, source, runner, original)
        remove_created_parents(created_parents, original)
        raise
    return worktree


def remove_created_parents(parents: list[Path], original: BaseException) -> None:
    try:
        for parent in reversed(parents):
            parent.rmdir()
    except OSError as cleanup:
        raise CowtreeError(
            CowtreeErrorCode.CLEANUP_FAILED,
            f"{original}; cleanup failed for new parent directory: {cleanup}",
        ) from original


def validate_branch(branch: str | None, source: Path, runner: CommandRunner) -> None:
    if branch is None:
        return
    checked = git_capture(runner, ["-C", str(source), "check-ref-format", "--branch", branch])
    if checked.removesuffix("\n") != branch:
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "branch must be a literal new branch name")
    if branch_exists(branch, source, runner):
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"branch already exists: {branch}")


def branch_exists(branch: str, source: Path, runner: CommandRunner) -> bool:
    result = runner.run(
        ["git", "-C", str(source), "show-ref", "--verify", "--quiet", f"refs/heads/{branch}"], check=False
    )
    if result.returncode not in (0, 1):
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, result.stderr)
    exists = result.returncode == 0
    return exists


def rollback_add(
    target: Path,
    branch: str | None,
    commit: str,
    source: Path,
    runner: CommandRunner,
    original: BaseException,
) -> None:
    try:
        registered = any(tree.path == target for tree in list_worktrees(runner, source))
        if registered:
            runner.run(["git", "-C", str(source), "worktree", "remove", "--force", "--force", str(target)])
        elif target.exists():
            target.rmdir()
        if branch is not None and branch_exists(branch, source, runner):
            # Compare-and-delete preserves a ref moved by an uncoordinated Git writer.
            runner.run(["git", "-C", str(source), "update-ref", "-d", f"refs/heads/{branch}", commit])
    except (CowtreeError, OSError) as cleanup:
        raise CowtreeError(
            CowtreeErrorCode.CLEANUP_FAILED,
            f"{original}; cleanup failed for {target}, branch {branch!r}: {cleanup}; "
            "inspect git worktree list and refs",
        ) from original


def list_all_worktrees(source: Path | None = None, runner: CommandRunner | None = None) -> list[Worktree]:
    """Return all Git worktrees in the repository, including ones created by Git."""
    runner = runner or CommandRunner()
    try:
        repo = source_root(runner, source)
        with repository_lock(runner, repo):
            worktrees = list_worktrees(runner, repo)
    except OSError as error:
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"list failed: {error}") from error
    return worktrees


def remove_worktree(
    path: Path,
    *,
    source: Path | None = None,
    force: bool = False,
    runner: CommandRunner | None = None,
) -> None:
    """Remove a linked worktree using Git's dirty and lock checks; preserve its branch."""
    runner = runner or CommandRunner()
    try:
        target = resolve_path(path)
        repo = source_root(runner, source)
        with repository_lock(runner, repo):
            args = ["git", "-C", str(repo), "worktree", "remove"]
            if force:
                args.append("--force")
            runner.run([*args, "--", str(target)])
    except OSError as error:
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"remove failed: {error}") from error


def inspect_path(path: Path, runner: CommandRunner | None = None) -> DoctorReport:
    """Probe native CoW support in an existing directory without requiring a Git repository."""
    report = doctor(path, runner)
    return report


def copy_tracked_files(source: Path, target: Path, files: list[TrackedFile]) -> None:
    for entry in files:
        src = source / entry.path
        dst = target / entry.path
        mode = src.lstat().st_mode
        dst.parent.mkdir(parents=True, exist_ok=True)
        if entry.mode == "120000" and stat.S_ISLNK(mode):
            os.symlink(os.readlink(src), dst)
        elif entry.mode in ("100644", "100755") and stat.S_ISREG(mode):
            executable = bool(mode & stat.S_IXUSR)
            if executable != (entry.mode == "100755"):
                raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, f"tracked executable mode changed: {entry.path}")
            clone_regular_file(src, dst)
        else:
            raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, f"tracked file type changed: {entry.path}")


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
        WorktreeAddRequest(source=source, path=target, branch="cow-branch"),
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
        add_worktree(WorktreeAddRequest(source=source, path=target, branch="old-branch", commitish="HEAD~1"), runner)
    assert error.value.code == CowtreeErrorCode.HEAD_MISMATCH

    assert not target.exists()
