from __future__ import annotations

from collections.abc import Mapping
from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor
from dataclasses import dataclass
import errno
import multiprocessing
import os
from pathlib import Path
from typing import Literal

import pytest

from cowtree import core
from cowtree.core import add_worktree, inspect_path, list_all_worktrees, remove_worktree
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.models import CommandResult, WorktreeAddRequest

from .conftest import Repository


@pytest.fixture
def cow_repository(repository: Repository) -> Repository:
    report = inspect_path(repository.path)
    expected = os.environ.get("COWTREE_EXPECT_SUPPORTED")
    if expected == "1":
        assert report.supported, report.reason
    if not report.supported:
        pytest.skip(report.reason or "filesystem does not support CoW")
    return repository


def request(
    repo: Repository, target: Path, branch: str | None = None, *, commitish: str = "HEAD", lock: bool = False
) -> WorktreeAddRequest:
    result = WorktreeAddRequest(path=target, source=repo.path, branch=branch, commitish=commitish, lock=lock)
    return result


def registered(repo: Repository) -> set[Path]:
    paths = {item.path for item in list_all_worktrees(repo.path)}
    return paths


def assert_absent(repo: Repository, target: Path, branch: str) -> None:
    assert not target.exists()
    assert target.resolve() not in registered(repo)
    assert repo.git("show-ref", "--verify", f"refs/heads/{branch}", check=False).returncode != 0


def assert_clean(repo: Repository, target: Path) -> None:
    assert repo.git("-C", str(target), "status", "--porcelain=v1", "--untracked-files=all").stdout == ""
    assert repo.git("-C", str(target), "diff", "HEAD", "--exit-code").returncode == 0


def test_invariant_filesystem_support_matches_expectation(tmp_path: Path) -> None:
    report = inspect_path(tmp_path)
    expected = os.environ.get("COWTREE_EXPECT_SUPPORTED")
    if expected is not None:
        assert expected in ("0", "1")
        assert report.supported == (expected == "1"), report


def test_invariant_unsupported_filesystem_leaves_no_state(repository: Repository, tmp_path: Path) -> None:
    if inspect_path(repository.path).supported:
        pytest.skip("requires a filesystem without CoW support")
    target = tmp_path / "unsupported"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request(repository, target, "unsupported"))
    assert error.value.code == CowtreeErrorCode.COW_UNAVAILABLE
    assert_absent(repository, target, "unsupported")


@pytest.mark.parametrize("name", ["with space", "tab\tname", "line\nname", "carriage\rname", "hyphen-name"])
def test_invariant_tracked_names_and_worktree_paths_round_trip(
    cow_repository: Repository, tmp_path: Path, name: str
) -> None:
    repo = cow_repository
    filename = f"{name}.txt"
    (repo.path / filename).write_bytes(b"a\r\nb\nc\x00\xff")
    repo.commit()
    target = tmp_path / name
    worktree = add_worktree(request(repo, target, "names"))
    assert worktree.path == target.resolve()
    assert (target / filename).read_bytes() == (repo.path / filename).read_bytes()
    assert target.resolve() in registered(repo)
    assert_clean(repo, target)
    remove_worktree(target, source=repo.path)
    assert not target.exists()


def test_invariant_non_utf8_names_round_trip(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    filename = os.fsdecode(b"raw-\xff-\xfe")
    try:
        (repo.path / filename).write_bytes(b"raw bytes\n")
    except OSError as error:
        if error.errno == errno.EILSEQ:
            pytest.skip("filesystem rejects non-UTF-8 filenames")
        raise
    repo.commit()
    target = tmp_path / f"target-{filename}"
    add_worktree(request(repo, target, "raw-bytes"))
    assert (target / filename).read_bytes() == b"raw bytes\n"
    assert target.resolve() in registered(repo)
    assert_clean(repo, target)
    remove_worktree(target, source=repo.path)


def test_invariant_checkout_preserves_modes_symlinks_and_isolation(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    (repo.path / "run").write_bytes(b"#!/bin/sh\nexit 0\n")
    (repo.path / "run").chmod(0o755)
    (repo.path / "file-link").symlink_to("file.txt")
    (repo.path / "dangling").symlink_to("missing")
    (repo.path / "directory-link").symlink_to("nested")
    (repo.path / "nested").mkdir()
    (repo.path / "nested" / "data").write_bytes(b"nested\n")
    repo.commit()
    target = tmp_path / "target"
    add_worktree(request(repo, target, "isolation"))
    assert (target / "run").stat().st_mode & 0o777 == 0o755
    for filename in ("file-link", "dangling", "directory-link"):
        assert os.readlink(target / filename) == os.readlink(repo.path / filename)
    assert_clean(repo, target)
    (target / "file.txt").write_bytes(b"target change\n")
    assert (repo.path / "file.txt").read_bytes() == b"original\n"
    (repo.path / "nested" / "data").write_bytes(b"source change\n")
    assert (target / "nested" / "data").read_bytes() == b"nested\n"


def test_invariant_clones_can_be_sources(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    first = tmp_path / "first"
    second = tmp_path / "second"
    add_worktree(request(repo, first, "first"))
    add_worktree(WorktreeAddRequest(path=second, source=first, branch="second"))
    assert_clean(repo, second)
    assert (second / "file.txt").read_bytes() == b"original\n"
    remove_worktree(first, source=repo.path)
    assert_clean(repo, second)
    remove_worktree(second, source=repo.path)
    assert registered(repo) == {repo.path}


def test_invariant_relative_paths_use_callers_directory(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository
    nested = tmp_path / "caller" / "nested"
    nested.mkdir(parents=True)
    monkeypatch.chdir(nested)
    worktree = add_worktree(request(repo, Path("../target"), "relative"))
    assert worktree.path == (nested.parent / "target").resolve()
    remove_worktree(Path("../target"), source=repo.path)
    assert registered(repo) == {repo.path}


@pytest.mark.parametrize("state", ["modified", "staged", "deleted", "assume-unchanged", "skip-worktree"])
def test_invariant_tracked_source_changes_are_refused(repository: Repository, tmp_path: Path, state: str) -> None:
    repo = repository
    path = repo.path / "file.txt"
    if state in ("assume-unchanged", "skip-worktree"):
        repo.git("update-index", f"--{state}", "--", "file.txt")
    if state == "deleted":
        path.unlink()
    else:
        path.write_bytes(b"changed\n")
    if state == "staged":
        repo.git("add", "file.txt")
    target = tmp_path / "refused"
    with pytest.raises(CowtreeError):
        add_worktree(request(repo, target, "refused"))
    assert_absent(repo, target, "refused")


def test_invariant_untracked_source_files_are_not_cloned(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    (repo.path / "untracked").write_bytes(b"private working data\n")
    target = tmp_path / "target"
    add_worktree(request(repo, target, "tracked-only"))
    assert not (target / "untracked").exists()
    assert_clean(repo, target)


def test_invariant_sparse_sources_are_refused(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    repo.git("sparse-checkout", "init", "--cone")
    target = tmp_path / "sparse"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request(repo, target, "sparse"))
    assert error.value.code == CowtreeErrorCode.SPARSE_CHECKOUT
    assert_absent(repo, target, "sparse")


def test_invariant_submodule_sources_are_refused(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    commit = repo.git("rev-parse", "HEAD").stdout.strip()
    repo.git("update-index", "--add", "--cacheinfo", f"160000,{commit},module")
    repo.git("commit", "-qm", "gitlink")
    (repo.path / "module").mkdir()
    target = tmp_path / "submodule"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request(repo, target, "submodule"))
    assert error.value.code == CowtreeErrorCode.SUBMODULE_UNSUPPORTED
    assert_absent(repo, target, "submodule")


def test_invariant_failed_revision_does_not_leave_a_branch(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    (repo.path / "file.txt").write_bytes(b"second\n")
    repo.commit()
    target = tmp_path / "old"
    with pytest.raises(CowtreeError) as error:
        add_worktree(request(repo, target, "failed", commitish="HEAD~1"))
    assert error.value.code == CowtreeErrorCode.HEAD_MISMATCH
    assert_absent(repo, target, "failed")


def test_invariant_existing_targets_are_preserved(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    target = tmp_path / "existing"
    target.mkdir()
    sentinel = target / "sentinel"
    sentinel.write_bytes(b"owned by caller\n")
    with pytest.raises(CowtreeError):
        add_worktree(request(repo, target, "existing"))
    assert sentinel.read_bytes() == b"owned by caller\n"
    assert target.resolve() not in registered(repo)
    assert repo.git("branch", "--list", "existing").stdout == ""


@pytest.mark.parametrize("locked", [False, True])
def test_invariant_checkout_failure_rolls_back_owned_state(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, locked: bool
) -> None:
    repo = cow_repository
    target = tmp_path / "failure"

    def fail_clone(source: Path, destination: Path) -> None:
        del source, destination
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected local clone I/O failure")

    monkeypatch.setattr(core, "clone_regular_file", fail_clone)
    with pytest.raises(CowtreeError, match="injected local clone I/O failure"):
        add_worktree(request(repo, target, "failure", lock=locked))
    assert_absent(repo, target, "failure")


def test_invariant_source_mutation_cannot_publish_a_dirty_clone(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository
    target = tmp_path / "mutating"
    original = core.clone_regular_file

    def mutate_then_clone(source: Path, destination: Path) -> None:
        source.write_bytes(b"changed during checkout\n")
        original(source, destination)

    monkeypatch.setattr(core, "clone_regular_file", mutate_then_clone)
    with pytest.raises(CowtreeError):
        add_worktree(request(repo, target, "mutating"))
    assert_absent(repo, target, "mutating")


@pytest.mark.parametrize("state", ["modified", "untracked", "locked"])
def test_invariant_removal_preserves_work_that_requires_force(
    cow_repository: Repository, tmp_path: Path, state: str
) -> None:
    repo = cow_repository
    target = tmp_path / "protected"
    add_worktree(request(repo, target, "protected", lock=state == "locked"))
    if state == "modified":
        (target / "file.txt").write_bytes(b"keep my edits\n")
    if state == "untracked":
        (target / "untracked").write_bytes(b"keep my work\n")
    with pytest.raises(CowtreeError):
        remove_worktree(target, source=repo.path)
    assert target.exists()
    assert target.resolve() in registered(repo)
    if state == "locked":
        with pytest.raises(CowtreeError):
            remove_worktree(target, source=repo.path, force=True)
        repo.git("worktree", "unlock", str(target))
    remove_worktree(target, source=repo.path, force=True)
    assert not target.exists()
    assert repo.git("show-ref", "--verify", "refs/heads/protected").returncode == 0


def test_invariant_large_checkout_has_exact_payload(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    payload = bytes(range(256)) * (4 * 1024)
    for index in range(256):
        folder = repo.path / f"directory-{index % 8}"
        folder.mkdir(exist_ok=True)
        (folder / f"file-{index}").write_bytes(str(index).encode() + b"\n")
    (repo.path / "large").write_bytes(payload * 8)
    repo.commit()
    target = tmp_path / "large"
    add_worktree(request(repo, target, "large"))
    assert (target / "large").read_bytes() == payload * 8
    for index in range(256):
        assert (target / f"directory-{index % 8}" / f"file-{index}").read_bytes() == str(index).encode() + b"\n"
    assert_clean(repo, target)


@dataclass(frozen=True)
class AddOutcome:
    path: Path | None
    error: CowtreeErrorCode | None


def add_concurrently(source: Path, target: Path, branch: str) -> AddOutcome:
    try:
        worktree = add_worktree(WorktreeAddRequest(path=target, source=source, branch=branch))
    except CowtreeError as error:
        outcome = AddOutcome(None, error.code)
        return outcome
    outcome = AddOutcome(worktree.path, None)
    return outcome


def remove_concurrently(source: Path, target: Path) -> None:
    remove_worktree(target, source=source)


def list_concurrently(source: Path) -> list[Path]:
    result = [item.path for item in list_all_worktrees(source)]
    assert len(result) == len(set(result))
    return result


def worker_count(kind: Literal["threads", "processes"]) -> int:
    count = int(os.environ.get("COWTREE_STRESS_WORKERS", "16" if kind == "threads" else "8"))
    assert 2 <= count <= 64, "COWTREE_STRESS_WORKERS must be between 2 and 64"
    return count


def make_executor(kind: Literal["threads", "processes"]) -> ThreadPoolExecutor | ProcessPoolExecutor:
    if kind == "threads":
        executor = ThreadPoolExecutor(max_workers=worker_count(kind))
        return executor
    processes = ProcessPoolExecutor(max_workers=worker_count(kind), mp_context=multiprocessing.get_context("spawn"))
    return processes


@pytest.mark.parametrize("kind", ["threads", "processes"])
def test_invariant_concurrent_operations_preserve_registration(
    cow_repository: Repository, tmp_path: Path, kind: Literal["threads", "processes"]
) -> None:
    repo = cow_repository
    targets = [tmp_path / f"parallel-{index}" for index in range(worker_count(kind))]
    with make_executor(kind) as executor:
        adds = [executor.submit(add_concurrently, repo.path, target, target.name) for target in targets]
        reads = [executor.submit(list_concurrently, repo.path) for _ in range(4)]
        outcomes = [future.result(timeout=60) for future in adds]
        for read in reads:
            read.result(timeout=60)
        assert {outcome.path for outcome in outcomes} == set(targets)
        assert all(outcome.error is None for outcome in outcomes)
        assert registered(repo) == {repo.path, *targets}
        for target in targets:
            assert_clean(repo, target)
        replacements = [tmp_path / f"replacement-{index}" for index in range(worker_count(kind))]
        removes = [executor.submit(remove_concurrently, repo.path, target) for target in targets]
        adds = [executor.submit(add_concurrently, repo.path, target, target.name) for target in replacements]
        reads = [executor.submit(list_concurrently, repo.path) for _ in range(4)]
        for future in removes:
            future.result(timeout=60)
        assert {future.result(timeout=60).path for future in adds} == set(replacements)
        for read in reads:
            read.result(timeout=60)
        assert registered(repo) == {repo.path, *replacements}
        removes = [executor.submit(remove_concurrently, repo.path, target) for target in replacements]
        for future in removes:
            future.result(timeout=60)
    assert registered(repo) == {repo.path}
    assert all(not target.exists() for target in [*targets, *replacements])


@pytest.mark.parametrize("kind", ["threads", "processes"])
def test_invariant_same_target_has_one_owner(
    cow_repository: Repository, tmp_path: Path, kind: Literal["threads", "processes"]
) -> None:
    repo = cow_repository
    target = tmp_path / "shared"
    with make_executor(kind) as executor:
        futures = [
            executor.submit(add_concurrently, repo.path, target, f"owner-{index}")
            for index in range(worker_count(kind))
        ]
        outcomes = [future.result(timeout=60) for future in futures]
    assert sum(outcome.path == target for outcome in outcomes) == 1
    assert sum(outcome.error is not None for outcome in outcomes) == worker_count(kind) - 1
    assert registered(repo) == {repo.path, target}
    branches = repo.git("for-each-ref", "--format=%(refname)", "refs/heads/owner-*").stdout.splitlines()
    assert len(branches) == 1
    assert_clean(repo, target)


def test_invariant_default_add_is_detached(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    worktree = add_worktree(request(repo, tmp_path / "detached"))
    assert worktree.detached
    assert worktree.branch is None
    assert_clean(repo, worktree.path)


def test_invariant_existing_branches_are_preserved(repository: Repository, tmp_path: Path) -> None:
    repo = repository
    repo.git("branch", "existing")
    original = repo.git("rev-parse", "existing").stdout
    target = tmp_path / "existing"
    with pytest.raises(CowtreeError):
        add_worktree(request(repo, target, "existing"))
    assert not target.exists()
    assert registered(repo) == {repo.path}
    assert repo.git("rev-parse", "existing").stdout == original


def test_invariant_failed_add_removes_only_owned_parent_directories(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository
    ancestor = tmp_path / "existing"
    ancestor.mkdir()
    sentinel = ancestor / "keep"
    sentinel.write_bytes(b"caller data\n")
    target = ancestor / "new" / "nested" / "target"

    def fail_clone(source: Path, destination: Path) -> None:
        del source, destination
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected clone failure")

    monkeypatch.setattr(core, "clone_regular_file", fail_clone)
    with pytest.raises(CowtreeError):
        add_worktree(request(repo, target, "parent-cleanup"))
    assert_absent(repo, target, "parent-cleanup")
    assert not (ancestor / "new").exists()
    assert sentinel.read_bytes() == b"caller data\n"


@pytest.mark.parametrize("operation", ["add", "remove"])
def test_invariant_unresolvable_paths_raise_typed_errors(
    repository: Repository, tmp_path: Path, operation: str
) -> None:
    loop = tmp_path / "loop"
    loop.symlink_to("loop")
    if operation == "add":
        with pytest.raises(CowtreeError):
            add_worktree(request(repository, loop / "target", "loop"))
    else:
        with pytest.raises(CowtreeError):
            remove_worktree(loop / "target", source=repository.path)
    assert registered(repository) == {repository.path}
    assert loop.is_symlink()


@pytest.mark.parametrize("failed_step", ["remove", "branch"])
def test_invariant_cleanup_failure_reports_surviving_owned_state(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failed_step: str
) -> None:
    repo = cow_repository
    target = tmp_path / "cleanup-failure"
    branch = "cleanup-owned-branch"

    class CleanupFailureRunner(CommandRunner):
        def run(
            self,
            argv: list[str],
            *,
            cwd: str | None = None,
            env: Mapping[str, str] | None = None,
            check: bool = True,
        ) -> CommandResult:
            if (failed_step == "remove" and "worktree" in argv and "remove" in argv) or (
                failed_step == "branch" and "update-ref" in argv and "-d" in argv
            ):
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected cleanup I/O failure")
            result = super().run(argv, cwd=cwd, env=env, check=check)
            return result

    def fail_clone(source: Path, destination: Path) -> None:
        del source, destination
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected clone I/O failure")

    monkeypatch.setattr(core, "clone_regular_file", fail_clone)
    with pytest.raises(CowtreeError) as error:
        add_worktree(request(repo, target, branch, lock=True), CleanupFailureRunner())
    assert error.value.code == CowtreeErrorCode.CLEANUP_FAILED
    assert "injected clone I/O failure" in error.value.message
    assert "injected cleanup I/O failure" in error.value.message
    assert str(target) in error.value.message
    assert branch in error.value.message
    assert target.exists() == (failed_step == "remove")
    assert repo.git("show-ref", "--verify", f"refs/heads/{branch}").returncode == 0


@pytest.mark.parametrize("branch_created", [False, True])
def test_invariant_failed_registration_removes_only_created_state(
    cow_repository: Repository, tmp_path: Path, branch_created: bool
) -> None:
    repo = cow_repository
    target = tmp_path / "owned-parent" / "nested" / "target"
    branch = "registration-owned-branch"
    message = "injected worktree registration failure"

    class RegistrationFailureRunner(CommandRunner):
        def run(
            self,
            argv: list[str],
            *,
            cwd: str | None = None,
            env: Mapping[str, str] | None = None,
            check: bool = True,
        ) -> CommandResult:
            if "worktree" in argv and "add" in argv:
                if branch_created:
                    super().run(["git", "-C", str(repo.path), "branch", branch, "HEAD"])
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, message)
            result = super().run(argv, cwd=cwd, env=env, check=check)
            return result

    with pytest.raises(CowtreeError) as error:
        add_worktree(request(repo, target, branch), RegistrationFailureRunner())
    assert error.value.code == CowtreeErrorCode.COMMAND_FAILED
    assert error.value.message == message
    assert_absent(repo, target, branch)
    assert not (tmp_path / "owned-parent").exists()


def test_invariant_rollback_preserves_a_branch_moved_by_another_writer(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository
    earlier = repo.git("rev-parse", "HEAD").stdout.strip()
    (repo.path / "file.txt").write_bytes(b"second commit\n")
    current = repo.commit()
    target = tmp_path / "target"
    branch = "externally-moved"

    def move_branch_then_fail(source: Path, destination: Path) -> None:
        del source, destination
        repo.git("update-ref", f"refs/heads/{branch}", earlier, current)
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected clone failure after external ref update")

    monkeypatch.setattr(core, "clone_regular_file", move_branch_then_fail)
    with pytest.raises(CowtreeError) as error:
        add_worktree(request(repo, target, branch))
    assert error.value.code == CowtreeErrorCode.CLEANUP_FAILED
    assert branch in error.value.message
    assert repo.git("rev-parse", branch).stdout.strip() == earlier
    assert repo.git("rev-parse", "HEAD").stdout.strip() == current
    assert not target.exists()
    assert registered(repo) == {repo.path}
