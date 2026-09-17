"""Reject impossible installations before recording intent or changing any path."""

from pathlib import Path

import pytest

from cowtree.errors import CowtreeError
from cowtree.install import Change, Installation, InstallRecord, fingerprint
from tests.test_install import object_entry


@pytest.mark.parametrize("retained", ["file", "child", "symlink", "empty-cache"])
def test_retained_namespace_conflicts_leave_no_journal(tmp_path: Path, retained: str) -> None:
    root = tmp_path / "leaf"
    root.mkdir()
    objects = tmp_path / "objects"
    objects.mkdir()
    remote = object_entry(directory=objects, data=b"remote")
    if retained == "file":
        (root / "private").write_bytes(b"retained")
        target = "private/remote"
    elif retained == "child":
        (root / "private").mkdir()
        (root / "private/untracked").write_bytes(b"retained")
        target = "private"
    elif retained == "symlink":
        (root / "private").symlink_to("missing", target_is_directory=True)
        target = "private/remote"
    else:
        (root / "private/empty-cache").mkdir(parents=True)
        target = "private"
    original = sorted(path.relative_to(root).as_posix() for path in root.rglob("*"))
    record = InstallRecord(
        root=root,
        changes=[
            Change(path="a-clean", before=None, after=remote),
            Change(path=target, before=None, after=remote),
        ],
    )
    with pytest.raises(CowtreeError):
        Installation.prepare(directory=tmp_path / "intent", record=record, objects=objects)
    assert not (tmp_path / "intent").exists()
    assert not (root / "a-clean").exists()
    assert sorted(path.relative_to(root).as_posix() for path in root.rglob("*")) == original


def test_effective_namespace_rejects_simultaneous_parent_and_child(tmp_path: Path) -> None:
    root = tmp_path / "leaf"
    root.mkdir()
    objects = tmp_path / "objects"
    objects.mkdir()
    remote = object_entry(directory=objects, data=b"remote")
    record = InstallRecord(
        root=root,
        changes=[
            Change(path="new", before=None, after=remote),
            Change(path="new/child", before=None, after=remote),
        ],
    )
    with pytest.raises(CowtreeError, match="namespace conflict"):
        Installation.prepare(directory=tmp_path / "intent", record=record, objects=objects)
    assert not (tmp_path / "intent").exists()
    assert list(root.iterdir()) == []


def test_complete_directory_replacement_preserves_reversibility(tmp_path: Path) -> None:
    root = tmp_path / "leaf"
    (root / "node").mkdir(parents=True)
    (root / "node/old").write_bytes(b"old")
    objects = tmp_path / "objects"
    objects.mkdir()
    remote = object_entry(directory=objects, data=b"remote")
    record = InstallRecord(
        root=root,
        changes=[
            Change(path="node/old", before=fingerprint(root / "node/old"), after=None),
            Change(path="node", before=None, after=remote),
        ],
    )
    installation = Installation.prepare(
        directory=tmp_path / "intent", record=record, objects=objects
    )
    installation.apply()
    assert (root / "node").read_bytes() == b"remote"
    installation.apply(rollback=True)
    assert (root / "node/old").read_bytes() == b"old"
