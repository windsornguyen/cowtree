"""Derived cache aliases must remain private when a snapshot becomes two leaves."""

from pathlib import Path
from typing import Literal

import pytest

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.leaves import Leaves
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary, source_repository


@pytest.mark.parametrize(
    "kind", ["absolute", "relative", "nested", "internal-absolute", "cycle", "source-bridge"]
)
def test_external_cache_aliases_are_rejected_before_a_workspace_is_published(
    tmp_path: Path,
    kind: Literal["absolute", "relative", "nested", "internal-absolute", "cycle", "source-bridge"],
) -> None:
    source = source_repository(tmp_path / "source")
    external = tmp_path / "external"
    external.mkdir()
    (external / "artifact").write_text("unchanged")
    cache = source / "cache"
    if kind == "nested":
        (cache / "alias").symlink_to(external)
    elif kind == "internal-absolute":
        (cache / "alias").symlink_to(cache / "artifact")
    elif kind == "cycle":
        (cache / "alias").symlink_to("alias")
    elif kind == "source-bridge":
        (source / "bridge").symlink_to(cache)
        (cache / "alias").symlink_to("../bridge/artifact")
    else:
        (cache / "artifact").unlink()
        cache.rmdir()
        cache.symlink_to(external if kind == "absolute" else Path("../external"))
    with pytest.raises(CowtreeError) as caught:
        Workspace.create(
            root=tmp_path / "store",
            source=source,
            binary=metadata_binary(),
            policy=PathPolicy(derived=("cache",)),
        )
    assert caught.value.code is CowtreeErrorCode.UNSUPPORTED_MODE
    assert "derived symlink" in caught.value.message
    assert not (tmp_path / "store").exists()
    assert (external / "artifact").read_text() == "unchanged"


def test_relative_cache_aliases_preserve_seed_and_sibling_bytes(tmp_path: Path) -> None:
    source = source_repository(tmp_path / "source")
    (source / "cache/alias").symlink_to("artifact")
    workspace = Workspace.create(
        root=tmp_path / "store",
        source=source,
        binary=metadata_binary(),
        policy=PathPolicy(derived=("cache",)),
    )
    first = Leaves(workspace).fork(tmp_path / "first")
    second = Leaves(workspace).fork(tmp_path / "second")
    (first.path / "cache/alias").write_text("changed")
    assert (first.path / "cache/artifact").read_text() == "changed"
    assert (source / "cache/artifact").read_bytes() == b"warm build"
    assert (second.path / "cache/alias").read_bytes() == b"warm build"


def test_dangling_relative_cache_chains_remain_private(tmp_path: Path) -> None:
    source = source_repository(tmp_path / "source")
    (source / "cache/alias").symlink_to("missing")
    (source / "cache/chain").symlink_to("alias")
    workspace = Workspace.create(
        root=tmp_path / "store",
        source=source,
        binary=metadata_binary(),
        policy=PathPolicy(derived=("cache",)),
    )
    first = Leaves(workspace).fork(tmp_path / "first")
    second = Leaves(workspace).fork(tmp_path / "second")
    (first.path / "cache/chain").write_text("private")
    assert (first.path / "cache/missing").read_text() == "private"
    assert not (source / "cache/missing").exists()
    assert not (second.path / "cache/missing").exists()
