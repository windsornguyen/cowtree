"""Attaching a branch must never take ownership of its reference."""

from pathlib import Path

import pytest

from cowtree.core import add_worktree
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import WorktreeAddRequest
from tests.conftest import Repository
from tests.test_cli import run_cli


@pytest.mark.parametrize("flags", [["-b", "new"], ["--detach"]])
def test_branch_policies_cannot_be_combined(flags: list[str], tmp_path: Path) -> None:
    res = run_cli(arguments=["add", "--branch", "old", *flags, "target"], cwd=tmp_path)
    assert res.returncode == 2
    assert not (tmp_path / "target").exists()


def test_existing_branch_attaches_without_rewriting(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    repo.git("branch", "resume", "HEAD")
    target = tmp_path / "resumed"
    res = run_cli(arguments=["add", "--branch", "resume", str(target)], cwd=repo.path)
    assert res.returncode == 0, res.stderr
    assert repo.git("-C", str(target), "symbolic-ref", "HEAD").stdout == "refs/heads/resume\n"
    repo.assert_clean(target=target)


def test_occupied_branch_remains_owned(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    branch = repo.git("symbolic-ref", "--short", "HEAD").stdout.strip()
    target = tmp_path / "occupied"
    with pytest.raises(CowtreeError):
        add_worktree(
            request=WorktreeAddRequest(path=target, source=repo.path, existing_branch=branch)
        )
    assert not target.exists()
    assert repo.git("symbolic-ref", "--short", "HEAD").stdout.strip() == branch


def test_ref_movement_during_clone_is_never_undone(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from cowtree.native import clone_regular_file

    repo = cow_repository
    earlier = repo.git("rev-parse", "HEAD").stdout.strip()
    (repo.path / "file.txt").write_text("next\n")
    current = repo.commit()
    repo.git("branch", "resume", current)

    def move_ref(source: Path, target: Path) -> None:
        repo.git("update-ref", "refs/heads/resume", earlier, current)
        clone_regular_file(source=source, target=target)

    monkeypatch.setattr("cowtree.core.clone_regular_file", move_ref)
    target = tmp_path / "moved"
    with pytest.raises(CowtreeError) as err:
        add_worktree(
            request=WorktreeAddRequest(path=target, source=repo.path, existing_branch="resume")
        )
    assert err.value.code is CowtreeErrorCode.HEAD_MISMATCH
    assert repo.git("rev-parse", "resume").stdout.strip() == earlier
    assert repo.registered == {repo.path}
    assert not target.exists()
