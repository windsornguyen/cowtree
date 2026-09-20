"""Committed forks leave the caller's index and working bytes untouched."""

from pathlib import Path

import pytest

from cowtree.core import add_worktree, list_all_worktrees
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import SourceMode, WorktreeAddRequest
from tests.conftest import Repository
from tests.test_cli import run_cli


@pytest.mark.parametrize("reference", ["HEAD", "HEAD~1"])
def test_committed_fork_ignores_private_edits(
    cow_repository: Repository, tmp_path: Path, reference: str
) -> None:
    repo = cow_repository
    (repo.path / "file.txt").write_bytes(b"committed second\n")
    repo.commit()
    (repo.path / "file.txt").write_bytes(b"staged private\n")
    repo.git("add", "file.txt")
    (repo.path / "file.txt").write_bytes(b"unstaged private\n")
    (repo.path / "untracked.txt").write_bytes(b"private untracked\n")
    index = repo.git("ls-files", "--stage", "-z").stdout
    status = repo.git("status", "--porcelain=v1", "-z").stdout
    target = tmp_path / "fork"
    res = run_cli(arguments=["add", "--committed", str(target), reference], cwd=repo.path)
    assert res.returncode == 0, res.stderr
    assert repo.git("ls-files", "--stage", "-z").stdout == index
    assert repo.git("status", "--porcelain=v1", "-z").stdout == status
    assert (repo.path / "file.txt").read_bytes() == b"unstaged private\n"
    assert not (target / "untracked.txt").exists()
    expected = repo.git("show", f"{reference}:file.txt").stdout.encode()
    assert (target / "file.txt").read_bytes() == expected
    repo.assert_clean(target=target)
    assert {tree.path for tree in list_all_worktrees(source=repo.path)} == {repo.path, target}
    (target / "file.txt").write_bytes(b"independent destination\n")
    assert (repo.path / "file.txt").read_bytes() == b"unstaged private\n"


def test_committed_branch_uses_its_own_tip(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    repo.git("branch", "resume", "HEAD")
    (repo.path / "file.txt").write_bytes(b"new main\n")
    repo.commit()
    target = tmp_path / "resume"
    res = run_cli(
        arguments=["add", "--committed", "--branch", "resume", str(target)], cwd=repo.path
    )
    assert res.returncode == 0, res.stderr
    assert (target / "file.txt").read_bytes() == b"original\n"
    assert repo.git("-C", str(target), "symbolic-ref", "HEAD").stdout == "refs/heads/resume\n"


def test_failed_clone_removes_seed_and_owned_branch(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = cow_repository

    def fail_clone(source: Path, target: Path) -> None:
        del source, target
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected clone failure")

    monkeypatch.setattr("cowtree.core.clone_regular_file", fail_clone)
    target = tmp_path / "failed"
    with pytest.raises(CowtreeError, match="injected clone failure"):
        add_worktree(
            request=WorktreeAddRequest(
                path=target,
                source=repo.path,
                branch="owned",
                source_mode=SourceMode.COMMIT,
            )
        )
    repo.assert_absent(target=target, branch="owned")
    assert repo.registered == {repo.path}
    assert not list(repo.path.parent.glob(".cowtree-seed-*"))


def test_committed_seed_applies_git_conversions(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    (repo.path / ".gitattributes").write_text("file.txt text eol=crlf\n")
    repo.commit()
    target = tmp_path / "converted"
    res = run_cli(arguments=["add", "--committed", str(target)], cwd=repo.path)
    assert res.returncode == 0, res.stderr
    assert (target / "file.txt").read_bytes() == b"original\r\n"
    repo.assert_clean(target=target)
