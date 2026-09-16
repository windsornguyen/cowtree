"""Exercise retained clones across repeated lifecycle changes."""

from pathlib import Path
import shutil

from cowtree.core import add_worktree, remove_worktree
from cowtree.types import WorktreeAddRequest

from .conftest import Repository


def test_invariant_missing_registration_does_not_block_an_independent_add(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    missing = tmp_path / "missing"
    target = tmp_path / "cafe\u0301"
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=missing))
    shutil.rmtree(missing)
    worktree = add_worktree(request=WorktreeAddRequest(source=repo.path, path=target))
    assert worktree.path.samefile(target)
    assert missing in repo.registered
    repo.assert_clean(target=target)
    remove_worktree(path=target, source=repo.path)
    repo.git("worktree", "prune", "--expire=now")
    assert repo.registered == {repo.path}


def test_invariant_descendant_survives_parent_removal(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    nested = repo.path.joinpath(*[f"dir{index}" for index in range(40)], "file")
    nested.parent.mkdir(parents=True)
    nested.write_bytes(b"deep source\n")
    repo.commit()
    parent = tmp_path / "parent"
    child = tmp_path / "child"
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=parent))
    add_worktree(request=WorktreeAddRequest(source=parent, path=child))
    remove_worktree(path=parent, source=repo.path)
    assert (child / nested.relative_to(repo.path)).read_bytes() == b"deep source\n"
    repo.assert_clean(target=child)
    (child / "file.txt").write_bytes(b"child edit\n")
    assert (repo.path / "file.txt").read_bytes() == b"original\n"
    remove_worktree(path=child, source=repo.path, force=True)
    assert repo.registered == {repo.path}


def test_invariant_repeated_target_reuse_leaves_only_retained_worktrees(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    retained = tmp_path / "retained"
    transient = tmp_path / "transient"
    add_worktree(request=WorktreeAddRequest(source=repo.path, path=retained))
    for index in range(20):
        add_worktree(request=WorktreeAddRequest(source=repo.path, path=transient))
        repo.assert_clean(target=transient)
        (transient / "file.txt").write_text(f"generation {index}\n")
        remove_worktree(path=transient, source=repo.path, force=True)
        assert not transient.exists()
        assert repo.registered == {repo.path, retained}
        assert (retained / "file.txt").read_bytes() == b"original\n"
        repo.assert_clean(target=retained)
    remove_worktree(path=retained, source=repo.path)
    assert repo.registered == {repo.path}
