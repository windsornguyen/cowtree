"""Leaf retirement preserves unrelated source and inherited descendants."""

from pathlib import Path

import pytest

from cowtree.errors import CowtreeError
from cowtree.lifecycle import DropRecord, Lifecycle
from cowtree.metadata import Metadata
from cowtree.seals import Seals

from .test_fork import manager


def test_drop_requires_private_source_consent_and_preserves_descendants(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    parent = leaves.fork(path=tmp_path / "parent")
    (parent.path / "file.txt").write_bytes(b"private")
    node = Seals(leaves.workspace).seal(identity=parent.id)
    child = leaves.fork(path=tmp_path / "child", node=node.id)
    lifecycle = Lifecycle(leaves.workspace)
    with pytest.raises(CowtreeError, match="private source changes"):
        lifecycle.drop(identity=parent.id)
    lifecycle.drop(identity=parent.id, force=True)
    assert not parent.path.exists()
    assert (child.path / "file.txt").read_bytes() == b"private"
    assert (leaves.workspace.config.source / "file.txt").read_bytes() == b"source bytes\n"


def test_interrupted_retirement_is_completed_before_new_operations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")

    def interrupt(
        self: Lifecycle,
        metadata: Metadata,
        directory: Path,
        record: DropRecord,
        descriptors: tuple[int, ...],
    ) -> None:
        del self, metadata, directory, record, descriptors
        raise InterruptedError("drop intent saved")

    with monkeypatch.context() as patch:
        patch.setattr(Lifecycle, "remove", interrupt)
        with pytest.raises(InterruptedError):
            Lifecycle(leaves.workspace).drop(identity=leaf.id)
    with leaves.workspace.session():
        assert leaves.records() == []
    assert not leaf.path.exists()
