"""Only a target matching the pinned source manifest can become a ready leaf."""

import os
from pathlib import Path
import stat

import pytest

from benchmarks.cargo_workspace import inventory, verify
from cowtree.errors import CowtreeError
from cowtree.exec import CommandRunner
from cowtree.leaves import Leaves
from cowtree.native import clone_regular_file
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary, source_repository


def workspace_fixture(root: Path) -> Workspace:
    source = source_repository(path=root / "source")
    (source / "src").mkdir()
    (source / "src/nested.rs").write_bytes(b"nested source bytes\n")
    runner = CommandRunner()
    runner.run(["git", "-C", str(source), "add", "src"])
    runner.run(["git", "-C", str(source), "commit", "-qm", "nested source"])
    result = Workspace.create(
        root=root / "workspace",
        source=source,
        binary=metadata_binary(),
        policy=PathPolicy(derived=("cache",), ephemeral=(".env",)),
    )
    return result


def corrupt_bytes(path: Path) -> None:
    """Change contents while preserving size and modification time."""
    before = path.stat()
    path.write_bytes(b"!" * before.st_size)
    os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))


@pytest.mark.parametrize(
    "damage", ["bytes", "delete", "extra", "file_symlink", "parent_symlink", "execute"]
)
def test_corrupt_frozen_sources_never_publish_ready_leaves(tmp_path: Path, damage: str) -> None:
    workspace = workspace_fixture(root=tmp_path)
    original = inventory(root=workspace.config.source)
    tree = workspace.root / "nodes" / workspace.config.initial / "tree"
    file = tree / "file.txt"
    if damage == "bytes":
        corrupt_bytes(path=file)
    elif damage == "delete":
        file.unlink()
    elif damage == "extra":
        (tree / "unexpected.rs").write_bytes(b"unpublished source")
    elif damage == "file_symlink":
        file.unlink()
        file.symlink_to("src/nested.rs")
    elif damage == "execute":
        file.chmod(stat.S_IMODE(file.stat().st_mode) ^ stat.S_IXUSR)
    else:
        assert damage == "parent_symlink"
        (tree / "src/nested.rs").unlink()
        (tree / "src").rmdir()
        (tree / "src").symlink_to(workspace.config.source / "src", target_is_directory=True)
    leaves = Leaves(workspace=workspace)
    target = tmp_path / "refused"
    with pytest.raises(CowtreeError):
        leaves.fork(path=target)
    assert leaves.records() == []
    leaves.recover()
    assert not target.exists()
    assert list((workspace.root / "operations").iterdir()) == []
    verify(root=workspace.config.source, expected=original)


def test_final_manifest_refuses_corruption_hidden_from_metadata_comparison(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    workspace = workspace_fixture(root=tmp_path)
    original = inventory(root=workspace.config.source)

    def corrupt_clone(source: Path, target: Path) -> None:
        clone_regular_file(source=source, target=target)
        if target.name == "file.txt":
            corrupt_bytes(path=target)

    leaves = Leaves(workspace=workspace)
    with monkeypatch.context() as patch:
        patch.setattr("cowtree.trees.clone_regular_file", corrupt_clone)
        with pytest.raises(CowtreeError, match="leaf differs from its source snapshot"):
            leaves.fork(path=tmp_path / "corrupt-target")
    assert leaves.records() == []
    leaves.recover()
    assert not (tmp_path / "corrupt-target").exists()
    verify(root=workspace.config.source, expected=original)


@pytest.mark.parametrize(
    "after_copy", ["copied_source", "future_source", "parent_symlink", "extra", "delete"]
)
def test_source_changes_during_copy_refuse_the_entire_fork(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, after_copy: str
) -> None:
    workspace = workspace_fixture(root=tmp_path)
    original = inventory(root=workspace.config.source)
    tree = workspace.root / "nodes" / workspace.config.initial / "tree"

    def change_source(source: Path, target: Path) -> None:
        clone_regular_file(source=source, target=target)
        if target.name != "file.txt":
            return
        if after_copy == "copied_source":
            corrupt_bytes(path=source)
        elif after_copy == "future_source":
            corrupt_bytes(path=tree / "src/nested.rs")
        elif after_copy == "parent_symlink":
            (tree / "src/nested.rs").unlink()
            (tree / "src").rmdir()
            (tree / "src").symlink_to(workspace.config.source / "src", target_is_directory=True)
        elif after_copy == "delete":
            source.unlink()
        else:
            assert after_copy == "extra"
            (tree / "unexpected.rs").write_bytes(b"late source")

    leaves = Leaves(workspace=workspace)
    with monkeypatch.context() as patch:
        patch.setattr("cowtree.trees.clone_regular_file", change_source)
        with pytest.raises(CowtreeError):
            leaves.fork(path=tmp_path / "mixed")
    assert leaves.records() == []
    leaves.recover()
    assert not (tmp_path / "mixed").exists()
    verify(root=workspace.config.source, expected=original)
