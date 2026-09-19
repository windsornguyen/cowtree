"""Private snapshots preserve lineage and keep caches out of Git history."""

from pathlib import Path

import pytest

from cowtree.errors import CowtreeError
from cowtree.git import GitRepository
from cowtree.nodes import Nodes
from cowtree.path_policy import check_aliases
from cowtree.projection import GitProjection
from cowtree.tree_types import PathPolicy

from .conftest import Repository


def test_private_nodes_preserve_cache_lineage_and_original_source(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    directory = tmp_path / "nodes"
    directory.mkdir()
    (repo.path / "cache").mkdir()
    (repo.path / "cache/artifact").write_bytes(b"warm cache")
    nodes = Nodes(
        directory=directory,
        projection=GitProjection(GitRepository(io=repo.runner, path=repo.path)),
        policy=PathPolicy(derived=("cache",)),
    )
    first = nodes.seal(source=repo.path)
    (repo.path / "file.txt").write_bytes(b"private edit")
    second = nodes.seal(source=repo.path, origins=first.source, parent=first.id)
    assert first.id != second.id
    assert second.origins == first.source
    assert second.source != first.source
    assert (directory / first.id / "tree/file.txt").read_bytes() == b"original\n"
    assert (directory / second.id / "tree/cache/artifact").read_bytes() == b"warm cache"
    assert repo.git("ls-tree", "--name-only", second.git_commit).stdout == "file.txt\n"
    nodes.verify(node=first)
    nodes.complete_refs()
    (directory / first.id / "tree/file.txt").write_bytes(b"corrupt")
    with pytest.raises(CowtreeError, match="snapshot source changed"):
        nodes.verify(node=first)


def test_case_and_unicode_aliases_cannot_create_independent_authority() -> None:
    for paths in ({"src/Foo", "src/foo"}, {"src/file", "SRC/other"}, {"café", "cafe\u0301"}):
        with pytest.raises(CowtreeError, match="filesystem aliases"):
            check_aliases(paths=paths)
