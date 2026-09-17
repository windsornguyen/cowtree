"""Ignore rules cannot accidentally publish credentials or hide tracked source."""

from pathlib import Path

import pytest

from cowtree.errors import CowtreeError
from cowtree.path_policy import working_policy
from cowtree.tree_types import PathPolicy
from cowtree.trees import clone_tree

from .conftest import Repository


def test_explicit_cache_inside_ignored_tree_excludes_its_siblings(
    cow_repository: Repository, tmp_path: Path
) -> None:
    repo = cow_repository
    (repo.path / ".gitignore").write_text("ignored/\n.env\n")
    repo.commit()
    (repo.path / "ignored/selected").mkdir(parents=True)
    (repo.path / "ignored/selected/cache").write_bytes(b"warm")
    (repo.path / "ignored/secret").write_bytes(b"private")
    (repo.path / ".env").write_bytes(b"credentials")
    policy = working_policy(source=repo.path, policy=PathPolicy(derived=("ignored/selected",)))
    target = tmp_path / "clone"
    clone_tree(source=repo.path, target=target, policy=policy)
    assert (target / "ignored/selected/cache").read_bytes() == b"warm"
    assert not (target / "ignored/secret").exists()
    assert not (target / ".env").exists()


def test_tracked_lockfiles_cannot_be_classified_as_derived(repository: Repository) -> None:
    (repository.path / "uv.lock").write_bytes(b"authoritative lock")
    repository.commit()
    with pytest.raises(CowtreeError, match="tracked source cannot be"):
        working_policy(source=repository.path, policy=PathPolicy(derived=("uv.lock",)))
