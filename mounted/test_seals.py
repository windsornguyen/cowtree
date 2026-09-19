"""Private checkpoints remain addressable and preserve later working edits."""

from pathlib import Path

import pytest

from cowtree.metadata import Metadata
from cowtree.seals import SealRecord, Seals
from cowtree.workspace_types import Node

from .test_fork import manager


def test_checkpoint_fork_inherits_unpublished_source_and_cache(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    parent = leaves.fork(path=tmp_path / "parent")
    (parent.path / "file.txt").write_bytes(b"private checkpoint")
    (parent.path / "cache/artifact").write_bytes(b"new build")
    seals = Seals(leaves.workspace)
    node = seals.seal(identity=parent.id)
    child = leaves.fork(path=tmp_path / "child", node=node.id)
    assert (child.path / "file.txt").read_bytes() == b"private checkpoint"
    assert (child.path / "cache/artifact").read_bytes() == b"new build"
    assert child.origins == parent.origins
    assert (leaves.workspace.root / "retained" / f"{node.id}.json").exists()
    seals.release(identity=node.id)
    assert not (leaves.workspace.root / "retained" / f"{node.id}.json").exists()


def test_seal_recovery_keeps_the_captured_snapshot_and_later_edits(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    (leaf.path / "file.txt").write_bytes(b"captured")
    seals = Seals(leaves.workspace)

    def interrupt(
        self: Seals, metadata: Metadata, directory: Path, record: SealRecord, node: Node
    ) -> None:
        del self, metadata, directory, record, node
        raise InterruptedError("captured before leaf update")

    with monkeypatch.context() as patch:
        patch.setattr(Seals, "complete", interrupt)
        with pytest.raises(InterruptedError):
            seals.seal(identity=leaf.id)
    (leaf.path / "file.txt").write_bytes(b"later edit")
    with leaves.workspace.session() as metadata:
        seals.recover_locked(metadata=metadata)
    updated = leaves.read(identity=leaf.id)
    assert (
        leaves.workspace.root / "nodes" / updated.node / "tree/file.txt"
    ).read_bytes() == b"captured"
    assert (leaf.path / "file.txt").read_bytes() == b"later edit"
