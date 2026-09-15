from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path

import pytest

from cowtree.core import add_worktree, clone_regular_file
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.types import CommandResult

from .conftest import Repository


@pytest.mark.parametrize("locked", [False, True])
def test_invariant_checkout_failure_rolls_back_owned_state(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, locked: bool
) -> None:
    repo = cow_repository
    target = tmp_path / "failure"

    def fail_clone(source: Path, target: Path) -> None:
        del source, target
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected local clone I/O failure")

    monkeypatch.setattr("cowtree.core.clone_regular_file", fail_clone)
    with pytest.raises(CowtreeError, match="injected local clone I/O failure"):
        add_worktree(request=repo.add_request(target=target, branch="failure", lock=locked))
    repo.assert_absent(target=target, branch="failure")


def test_invariant_source_mutation_cannot_publish_a_dirty_clone(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository
    target = tmp_path / "mutating"
    original = clone_regular_file

    def mutate_then_clone(source: Path, target: Path) -> None:
        source.write_bytes(b"changed during checkout\n")
        original(source=source, target=target)

    monkeypatch.setattr("cowtree.core.clone_regular_file", mutate_then_clone)
    with pytest.raises(CowtreeError):
        add_worktree(request=repo.add_request(target=target, branch="mutating"))
    repo.assert_absent(target=target, branch="mutating")


def test_invariant_failed_add_removes_only_owned_parent_directories(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository
    ancestor = tmp_path / "existing"
    ancestor.mkdir()
    sentinel = ancestor / "keep"
    sentinel.write_bytes(b"caller data\n")
    target = ancestor / "new" / "nested" / "target"

    def fail_clone(source: Path, target: Path) -> None:
        del source, target
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected clone failure")

    monkeypatch.setattr("cowtree.core.clone_regular_file", fail_clone)
    with pytest.raises(CowtreeError):
        add_worktree(request=repo.add_request(target=target, branch="parent-cleanup"))
    repo.assert_absent(target=target, branch="parent-cleanup")
    assert not (ancestor / "new").exists()
    assert sentinel.read_bytes() == b"caller data\n"


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
            if failed_step == "remove" and "worktree" in argv and "remove" in argv:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected cleanup I/O failure")
            if failed_step == "branch" and "update-ref" in argv and "-d" in argv:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected cleanup I/O failure")
            result = super().run(argv, cwd=cwd, env=env, check=check)
            return result

    def fail_clone(source: Path, target: Path) -> None:
        del source, target
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected clone I/O failure")

    monkeypatch.setattr("cowtree.core.clone_regular_file", fail_clone)
    with pytest.raises(CowtreeError) as error:
        add_worktree(
            request=repo.add_request(target=target, branch=branch, lock=True),
            runner=CleanupFailureRunner(),
        )
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
        add_worktree(
            request=repo.add_request(target=target, branch=branch),
            runner=RegistrationFailureRunner(),
        )
    assert error.value.code == CowtreeErrorCode.COMMAND_FAILED
    assert error.value.message == message
    repo.assert_absent(target=target, branch=branch)
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

    def move_branch_then_fail(source: Path, target: Path) -> None:
        del source, target
        repo.git("update-ref", f"refs/heads/{branch}", earlier, current)
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, "injected clone failure after external ref update"
        )

    monkeypatch.setattr("cowtree.core.clone_regular_file", move_branch_then_fail)
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=repo.add_request(target=target, branch=branch))
    assert error.value.code == CowtreeErrorCode.CLEANUP_FAILED
    assert branch in error.value.message
    assert repo.git("rev-parse", branch).stdout.strip() == earlier
    assert repo.git("rev-parse", "HEAD").stdout.strip() == current
    assert not target.exists()
    assert repo.registered == {repo.path}
