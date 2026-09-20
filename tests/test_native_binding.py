"""The installed Python binding preserves the native worktree contract."""

from pathlib import Path

import pytest

from cowtree.core import add_worktree, inspect_path, list_all_worktrees, remove_worktree
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import SourceMode, WorktreeAddRequest
from tests.conftest import Repository


def test_native_binding_preserves_private_edits(cow_repository: Repository, tmp_path: Path) -> None:
    repo = cow_repository
    target = tmp_path / "native tree"
    request = WorktreeAddRequest(path=target, source=repo.path, branch="native/topic")
    tree = add_worktree(request=request)
    assert tree.path == target.resolve()
    assert tree.branch == "refs/heads/native/topic"
    assert inspect_path(path=target).supported
    assert tree in list_all_worktrees(source=repo.path)
    (target / "file.txt").write_bytes(b"private edit\n")
    assert (repo.path / "file.txt").read_bytes() == b"original\n"
    with pytest.raises(CowtreeError) as error:
        remove_worktree(path=target, source=repo.path)
    assert error.value.code is CowtreeErrorCode.COMMAND_FAILED
    assert (target / "file.txt").read_bytes() == b"private edit\n"
    remove_worktree(path=target, source=repo.path, force=True)
    assert not target.exists()
    assert repo.git("rev-parse", "native/topic").stdout == repo.git("rev-parse", "HEAD").stdout


def test_native_binding_preserves_committed_source_policy(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    (repo.path / "file.txt").write_bytes(b"staged edit\n")
    repo.git("add", "file.txt")
    (repo.path / "file.txt").write_bytes(b"unstaged edit\n")
    before = repo.git("diff", "--cached").stdout
    request = WorktreeAddRequest(
        path=tmp_path / "committed",
        source=repo.path,
        source_mode=SourceMode.COMMIT,
    )
    tree = add_worktree(request=request)
    assert tree.detached
    assert (tree.path / "file.txt").read_bytes() == b"original\n"
    assert (repo.path / "file.txt").read_bytes() == b"unstaged edit\n"
    assert repo.git("diff", "--cached").stdout == before
    remove_worktree(path=tree.path, source=repo.path)


def test_native_binding_rejects_existing_destinations(repository: Repository) -> None:
    request = WorktreeAddRequest(path=repository.path, source=repository.path)
    with pytest.raises(CowtreeError) as error:
        add_worktree(request=request)
    assert error.value.code is CowtreeErrorCode.INVALID_ARGUMENTS
    assert (repository.path / "file.txt").read_bytes() == b"original\n"
