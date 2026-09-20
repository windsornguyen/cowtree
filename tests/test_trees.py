"""Real filesystem contracts for source, cache, and ephemeral trees."""

import os
from pathlib import Path
from typing import NoReturn, cast

import pytest

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.native import clone_regular_file
from cowtree.tree_types import CaptureMode, DerivedHardlinks, PathClass, PathPolicy
from cowtree.trees import clone_tree, populate_tree, scan_tree

from .conftest import Repository


def test_untyped_capture_mode_cannot_disable_verification(tmp_path: Path) -> None:
    source, target = tmp_path / "source", tmp_path / "target"
    source.mkdir()
    target.mkdir()
    (source / "file").write_bytes(b"preserved source")
    capture = cast(CaptureMode, "metadata")
    with pytest.raises(CowtreeError, match="invalid capture mode"):
        populate_tree(source=source, target=target, policy=PathPolicy(), capture=capture)
    with pytest.raises(CowtreeError, match="invalid capture mode"):
        scan_tree(root=source, policy=PathPolicy(), capture=capture)
    assert list(target.iterdir()) == []
    assert (source / "file").read_bytes() == b"preserved source"


def test_metadata_capture_keeps_source_classification_without_reading_contents(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "source.rs").write_bytes(b"source bytes")

    def unexpected_hash() -> NoReturn:
        raise AssertionError("metadata capture read source contents")

    monkeypatch.setattr("cowtree.trees.hashlib.sha256", unexpected_hash)
    entries = scan_tree(root=tmp_path, policy=PathPolicy(), capture=CaptureMode.METADATA)
    assert len(entries) == 1
    assert entries[0].classification is PathClass.SOURCE
    assert entries[0].digest is None
    assert entries[0].identity is not None


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


def test_opted_in_cache_hardlinks_become_independent_native_clones(
    cow_repository: Repository, tmp_path: Path
) -> None:
    source = cow_repository.path
    cache = source / "target"
    cache.mkdir()
    original = cache / "artifact"
    original.write_bytes(b"compiled cache")
    original.chmod(0o751)
    os.link(original, cache / "alias")
    metadata = original.stat()
    policy = PathPolicy(derived=("target",), derived_hardlinks=DerivedHardlinks.CLONE)
    target = tmp_path / "clone"
    clone_tree(source=source, target=target, policy=policy)
    first, second = target / "target/artifact", target / "target/alias"
    assert first.read_bytes() == second.read_bytes() == b"compiled cache"
    assert len({metadata.st_ino, first.stat().st_ino, second.stat().st_ino}) == 3
    assert first.stat().st_nlink == second.stat().st_nlink == 1
    assert first.stat().st_mode == metadata.st_mode
    assert first.stat().st_mtime_ns == metadata.st_mtime_ns
    first.write_bytes(b"private output")
    assert second.read_bytes() == original.read_bytes() == b"compiled cache"
    original.write_bytes(b"changed source cache")
    assert (cache / "alias").read_bytes() == b"changed source cache"
    assert second.read_bytes() == b"compiled cache"
    assert first.read_bytes() == b"private output"


def test_cache_opt_in_never_admits_source_hardlinks(tmp_path: Path) -> None:
    (tmp_path / "source.rs").write_bytes(b"source")
    (tmp_path / "target").mkdir()
    os.link(tmp_path / "source.rs", tmp_path / "target/alias")
    policy = PathPolicy(derived=("target",), derived_hardlinks=DerivedHardlinks.CLONE)
    with pytest.raises(CowtreeError, match=r"hard-linked file: source\.rs"):
        scan_tree(root=tmp_path, policy=policy)


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
        metadata = target.stat()
        target.write_bytes(b"!" * metadata.st_size)
        os.utime(target, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))

    monkeypatch.setattr("cowtree.trees.clone_regular_file", wrong_capture)
    target = tmp_path / "copy"
    with pytest.raises(CowtreeError):
        clone_tree(source=cow_repository.path, target=target, policy=PathPolicy())
    assert not target.exists()
    assert (cow_repository.path / "file.txt").read_bytes() == b"original\n"
