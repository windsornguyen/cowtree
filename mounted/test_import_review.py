"""Initial import preserves the existing Git source-mode contract."""

from pathlib import Path

import pytest

from cowtree.metadata_types import EntryKind
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary, source_repository


@pytest.mark.parametrize("mode", [0o654, 0o645], ids=("group_execute", "other_execute"))
def test_import_uses_owner_execute_bit_for_git_file_kind(tmp_path: Path, mode: int) -> None:
    source = source_repository(path=tmp_path / "source")
    (source / "file.txt").chmod(mode)
    workspace = Workspace.create(
        root=tmp_path / "store", source=source, binary=metadata_binary(), policy=PathPolicy()
    )
    node = workspace.nodes.read(identity=workspace.config.initial)
    assert node.source["file.txt"].kind is EntryKind.FILE
    assert (source / "file.txt").read_bytes() == b"source bytes\n"
