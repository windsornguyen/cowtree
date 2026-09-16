"""Managed worktree identity survives alternate filesystem path spellings."""

from __future__ import annotations

from pathlib import Path
import shutil
import unicodedata

import pytest

from cowtree.core import list_all_worktrees
from cowtree.workspace import Workspace
from cowtree.workspace_types import MetadataError, WorkspaceLeaf


def test_unicode_worktree_reopen_remove_uses_filesystem_identity(
    workspace: Workspace,
    tmp_path: Path,
) -> None:
    target = tmp_path / "caf\u00e9"
    leaf = workspace.add(target)
    opened = Workspace.open(
        source=workspace.source,
        authority=workspace.metadata.authority,
        binary=workspace.metadata.binary,
    )
    alternate = tmp_path / unicodedata.normalize("NFD", target.name)
    alias = WorkspaceLeaf(leaf=leaf.leaf, path=alternate)
    if alternate.exists():
        assert alternate.samefile(leaf.path)
        opened.remove(alias)
    else:
        with pytest.raises(MetadataError) as error:
            opened.remove(alias)
        assert error.value.code == "binding_changed"
        assert leaf.path.exists()
        assert leaf.leaf in opened.leaves()
        opened.remove(leaf)
    assert not leaf.path.exists()
    assert not alternate.exists()
    assert opened.leaves() == ()
    assert {tree.path for tree in list_all_worktrees(source=workspace.source)} == {workspace.source}


def test_remove_rejects_different_existing_directory(workspace: Workspace, tmp_path: Path) -> None:
    leaf = workspace.add(tmp_path / "reader")
    other = tmp_path / "other"
    other.mkdir()
    with pytest.raises(MetadataError) as error:
        workspace.remove(WorkspaceLeaf(leaf=leaf.leaf, path=other))
    assert error.value.code == "binding_changed"
    assert leaf.path.exists()
    assert other.exists()
    assert leaf.leaf in workspace.leaves()
    workspace.remove(leaf)
    assert other.exists()


def test_remove_cleans_a_missing_registered_directory(workspace: Workspace, tmp_path: Path) -> None:
    leaf = workspace.add(tmp_path / "missing")
    shutil.rmtree(leaf.path)
    workspace.remove(leaf)
    assert workspace.leaves() == ()
    assert {tree.path for tree in list_all_worktrees(source=workspace.source)} == {workspace.source}
