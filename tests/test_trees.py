"""Real filesystem contracts for source, cache, and ephemeral trees."""

import os
from pathlib import Path

import pytest

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.native import clone_regular_file
from cowtree.tree_types import PathClass, PathPolicy
from cowtree.trees import clone_tree, populate_tree, scan_tree

from .conftest import Repository


def test_tree_inherits_cache_without_ephemeral_state(
    cow_repository: Repository, tmp_path: Path
) -> None:
    source = cow_repository.path
    (source / "cache").mkdir()
    (source / "cache" / "empty").mkdir()
    cached = source / "cache" / "artifact"
    cached.write_bytes(b"compiled bytes")
    timestamp = 1_700_000_000_000_000_000
    os.utime(cached, ns=(timestamp, timestamp))
    (source / "secret").write_text("never inherit")
    (source / "link").symlink_to("file.txt")
    policy = PathPolicy(derived=("cache",), ephemeral=("secret",))
    target = tmp_path / "copy"
    entries = clone_tree(source=source, target=target, policy=policy)
    assert not (target / ".git").exists()
    assert not (target / "secret").exists()
    assert (target / "cache" / "empty").is_dir()
    assert (target / "cache" / "artifact").stat().st_mtime_ns == cached.stat().st_mtime_ns
    assert (target / "cache" / "artifact").read_bytes() == b"compiled bytes"
    assert os.readlink(target / "link") == "file.txt"
    assert (
        next(entry for entry in entries if entry.path == "cache/artifact").classification
        is PathClass.DERIVED
    )
    (target / "cache" / "artifact").write_bytes(b"changed")
    assert cached.read_bytes() == b"compiled bytes"


@pytest.mark.parametrize("kind", ["hardlink", "fifo"])
def test_tree_rejects_unsupported_entries(tmp_path: Path, kind: str) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "file").write_bytes(b"source")
    if kind == "hardlink":
        os.link(source / "file", source / "other")
    else:
        os.mkfifo(source / "pipe")
    with pytest.raises(CowtreeError) as caught:
        clone_tree(source=source, target=tmp_path / "copy", policy=PathPolicy())
    assert caught.value.code is CowtreeErrorCode.UNSUPPORTED_MODE
    assert not (tmp_path / "copy").exists()


@pytest.mark.parametrize("prefix", ["", "../cache", "/cache", "cache/", ".git", "a//b"])
def test_policy_rejects_ambiguous_prefixes(tmp_path: Path, prefix: str) -> None:
    with pytest.raises(CowtreeError) as caught:
        scan_tree(root=tmp_path, policy=PathPolicy(derived=(prefix,)))
    assert caught.value.code is CowtreeErrorCode.INVALID_ARGUMENTS


def test_policy_requires_disjoint_classes(tmp_path: Path) -> None:
    with pytest.raises(CowtreeError):
        scan_tree(root=tmp_path, policy=PathPolicy(derived=("cache",), ephemeral=("cache/tmp",)))


def test_ephemeral_subtree_is_not_traversed(tmp_path: Path) -> None:
    (tmp_path / "runtime").mkdir()
    os.mkfifo(tmp_path / "runtime" / "pipe")
    entries = scan_tree(root=tmp_path, policy=PathPolicy(ephemeral=("runtime",)))
    assert entries == ()


def test_owned_population_preserves_git_metadata_and_refuses_other_contents(
    cow_repository: Repository, tmp_path: Path
) -> None:
    target = tmp_path / "registered"
    target.mkdir()
    control = target / ".git"
    control.write_bytes(b"owned registration")
    populate_tree(source=cow_repository.path, target=target, policy=PathPolicy())
    assert control.read_bytes() == b"owned registration"
    assert (target / "file.txt").read_bytes() == b"original\n"
    with pytest.raises(CowtreeError, match="contains data"):
        populate_tree(source=cow_repository.path, target=target, policy=PathPolicy())
    assert control.read_bytes() == b"owned registration"


def test_tree_never_publishes_bytes_different_from_capture(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def wrong_capture(source: Path, target: Path) -> None:
        clone_regular_file(source=source, target=target)
        target.write_bytes(b"intermediate source value")

    monkeypatch.setattr("cowtree.trees.clone_regular_file", wrong_capture)
    target = tmp_path / "copy"
    with pytest.raises(CowtreeError):
        clone_tree(source=cow_repository.path, target=target, policy=PathPolicy())
    assert not target.exists()
    assert (cow_repository.path / "file.txt").read_bytes() == b"original\n"
