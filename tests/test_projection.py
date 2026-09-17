"""Git projection preserves the caller's index and excludes cache payload."""

from pathlib import Path

import pytest

from cowtree.errors import CowtreeError
from cowtree.git import GitRepository
from cowtree.projection import GitProjection

from .conftest import Repository


def test_source_projection_does_not_stage_cache_or_mutate_the_callers_index(
    repository: Repository, tmp_path: Path
) -> None:
    repo = repository
    tree = tmp_path / "tree"
    tree.mkdir()
    (tree / "file.txt").write_bytes(b"new source\n")
    (tree / "cache").write_bytes(b"never commit")
    (tree / "line\nname").write_bytes(b"newline")
    (repo.path / "staged").write_bytes(b"caller edit")
    repo.git("add", "staged")
    before = (repo.path / ".git/index").read_bytes()
    head = repo.git("rev-parse", "HEAD").stdout.strip()
    projection = GitProjection(repository=GitRepository(io=repo.runner, path=repo.path))
    commit = projection.create(tree=tree, paths=("file.txt", "line\nname"), parents=(head,))
    assert (
        projection.create(
            tree=tree, paths=("file.txt", "line\nname"), parents=(head,), reuse=commit
        )
        == commit
    )
    with pytest.raises(CowtreeError, match="differs from validated"):
        projection.create(tree=tree, paths=("file.txt",), parents=(head,), reuse=commit)
    assert (repo.path / ".git/index").read_bytes() == before
    assert repo.git("rev-parse", "HEAD").stdout.strip() == head
    assert repo.git("show", f"{commit}:file.txt").stdout == "new source\n"
    assert repo.git("ls-tree", "--name-only", "-z", commit).stdout == "file.txt\0line\nname\0"
    projection.set_ref(reference="refs/cowtree/nodes/test", commit=commit)
    projection.set_ref(reference="refs/cowtree/nodes/test", commit=commit)
    with pytest.raises(CowtreeError):
        projection.set_ref(reference="refs/heads/main", commit=commit)
    with pytest.raises(CowtreeError):
        projection.set_ref(reference="refs/cowtree/nodes/test", commit=head)
    with pytest.raises(CowtreeError):
        projection.remove_ref(reference="refs/cowtree/nodes/test", expected=head)
    projection.remove_ref(reference="refs/cowtree/nodes/test", expected=commit)
    projection.remove_ref(reference="refs/cowtree/nodes/test", expected=commit)


def test_projection_uses_the_candidates_checkout_attributes(
    repository: Repository, tmp_path: Path
) -> None:
    repo = repository
    tree = tmp_path / "tree"
    tree.mkdir()
    (tree / ".gitattributes").write_text("file.txt text eol=crlf\n")
    (tree / "file.txt").write_bytes(b"one\r\ntwo\r\n")
    projection = GitProjection(repository=GitRepository(io=repo.runner, path=repo.path))
    commit = projection.create(tree=tree, paths=(".gitattributes", "file.txt"))
    assert repo.git("show", f"{commit}:file.txt").stdout == "one\ntwo\n"
