"""A missing CoW primary must be rejected before binding or import journals exist."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from cowtree.metadata_client import MetadataClient
from cowtree.workspace_types import MetadataError, WireRecord


def test_unsupported_filesystem_leaves_no_state_in_metadata(tmp_path: Path) -> None:
    """Run explicitly on the negative filesystem arms alongside the native Git checks."""
    if os.environ.get("COWTREE_EXPECT_SUPPORTED") != "0":
        pytest.skip("requires the explicit unsupported-filesystem matrix arm")
    binary = Path(os.environ["COWTREE_METADATA_BINARY"])
    client = MetadataClient(authority=tmp_path / "authority", binary=binary)
    client.call({"op": "init"}, "done")
    leaf = client.call({"op": "create_leaf"}, "leaf")
    tree = tmp_path / "tree"
    tree.mkdir()
    with pytest.raises(MetadataError, match="native copy-on-write") as failure:
        client.call(
            {"op": "bind_tree", "leaf": leaf, "path": str(tree), "version": 0}, "installation"
        )
    assert failure.value.code == "unsupported_filesystem"
    with pytest.raises(MetadataError) as unbound:
        client.call({"op": "binding", "leaf": leaf}, "installation")
    assert unbound.value.code == "unbound_leaf"
    assert WireRecord.parse(client.call({"op": "tip"}, "tip")).number("version") == 0
