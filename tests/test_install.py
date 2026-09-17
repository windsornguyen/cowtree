"""Installation retries preserve old descriptors and refuse intervening edits."""

import hashlib
import os
from pathlib import Path

import pytest

from cowtree.errors import CowtreeError
from cowtree.install import Change, Installation, InstallRecord, fingerprint
from cowtree.metadata_types import Entry, EntryKind

from .conftest import Repository


def object_entry(directory: Path, data: bytes) -> Entry:
    digest = hashlib.sha256(data).hexdigest()
    (directory / digest).write_bytes(data)
    result = Entry(object=digest, kind=EntryKind.FILE)
    return result


def test_installation_replay_preserves_later_edits_and_old_descriptors(
    cow_repository: Repository, tmp_path: Path
) -> None:
    root = cow_repository.path
    objects = tmp_path / "objects"
    objects.mkdir()
    after = object_entry(directory=objects, data=b"published")
    target = root / "file.txt"
    record = InstallRecord(
        root=root, changes=[Change(path="file.txt", before=fingerprint(path=target), after=after)]
    )
    install = Installation.prepare(
        directory=tmp_path / "transaction", record=record, objects=objects
    )
    with target.open("rb") as original:
        install.apply()
        assert target.read_bytes() == b"published"
        assert original.read() == b"original\n"
    reopened = Installation.open(directory=install.directory)
    reopened.apply()
    target.write_bytes(b"later edit")
    with pytest.raises(CowtreeError, match="installation conflict"):
        reopened.apply(rollback=True)
    assert target.read_bytes() == b"later edit"
    assert (install.directory / "install.json").is_file()


@pytest.mark.parametrize("after_replace", [False, True])
def test_interrupted_installation_can_replay_then_restore_before_images(
    cow_repository: Repository, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, after_replace: bool
) -> None:
    root = cow_repository.path
    objects = tmp_path / "objects"
    objects.mkdir()
    after = object_entry(directory=objects, data=b"next")
    changes = [
        Change(path="file.txt", before=fingerprint(path=root / "file.txt"), after=after),
        Change(path="new/nested", before=None, after=after),
    ]
    install = Installation.prepare(
        directory=tmp_path / "transaction",
        record=InstallRecord(root=root, changes=changes),
        objects=objects,
    )
    replace = os.replace

    def interrupt_after_replace(source: Path, target: Path) -> None:
        if after_replace:
            replace(source, target)
        raise InterruptedError("injected interruption")

    with monkeypatch.context() as patch:
        patch.setattr("cowtree.install.os.replace", interrupt_after_replace)
        with pytest.raises(InterruptedError):
            install.apply()
    reopened = Installation.open(directory=install.directory)
    reopened.apply()
    assert (root / "new/nested").read_bytes() == b"next"
    reopened.apply(rollback=True)
    assert (root / "file.txt").read_bytes() == b"original\n"
    assert not (root / "new").exists()
    reopened.finish()
    assert not reopened.directory.exists()


def test_file_directory_namespace_change_is_reversible(
    cow_repository: Repository, tmp_path: Path
) -> None:
    root = cow_repository.path
    objects = tmp_path / "objects"
    objects.mkdir()
    after = object_entry(directory=objects, data=b"child")
    changes = [
        Change(path="file.txt", before=fingerprint(path=root / "file.txt"), after=None),
        Change(path="file.txt/child", before=None, after=after),
    ]
    install = Installation.prepare(
        directory=tmp_path / "transaction",
        record=InstallRecord(root=root, changes=changes),
        objects=objects,
    )
    install.apply()
    assert (root / "file.txt/child").read_bytes() == b"child"
    install.apply(rollback=True)
    assert (root / "file.txt").read_bytes() == b"original\n"


def test_symlink_parent_is_not_an_installation_directory(
    cow_repository: Repository, tmp_path: Path
) -> None:
    root = cow_repository.path
    (root / "alias").symlink_to(tmp_path, target_is_directory=True)
    record = InstallRecord(
        root=root, changes=[Change(path="alias/outside", before=None, after=None)]
    )
    with pytest.raises(CowtreeError, match="symlink parent"):
        Installation.prepare(directory=tmp_path / "transaction", record=record, objects=tmp_path)
    assert not (tmp_path / "transaction").exists()


def test_replaced_root_cannot_receive_an_old_installation(
    cow_repository: Repository, tmp_path: Path
) -> None:
    root = cow_repository.path
    record = InstallRecord(root=root, changes=[])
    install = Installation.prepare(
        directory=tmp_path / "transaction", record=record, objects=tmp_path
    )
    root.rename(tmp_path / "old-root")
    root.mkdir()
    with pytest.raises(CowtreeError, match="root identity changed"):
        install.apply()
    assert list(root.iterdir()) == []


def test_changed_source_gets_a_fresh_timestamp_even_for_an_old_object(
    cow_repository: Repository, tmp_path: Path
) -> None:
    root = cow_repository.path
    target = root / "file.txt"
    previous = target.stat().st_mtime_ns
    objects = tmp_path / "objects"
    objects.mkdir()
    after = object_entry(directory=objects, data=b"changed source")
    old = 1_000_000_000_000_000_000
    os.utime(objects / after.object, ns=(old, old))
    record = InstallRecord(
        root=root, changes=[Change(path="file.txt", before=fingerprint(path=target), after=after)]
    )
    installation = Installation.prepare(
        directory=tmp_path / "transaction", record=record, objects=objects
    )
    installation.apply()
    assert target.stat().st_mtime_ns >= previous


def test_corrupt_object_cannot_change_the_working_tree(
    cow_repository: Repository, tmp_path: Path
) -> None:
    root = cow_repository.path
    objects = tmp_path / "objects"
    objects.mkdir()
    after = object_entry(directory=objects, data=b"expected")
    (objects / after.object).write_bytes(b"corrupt")
    record = InstallRecord(
        root=root,
        changes=[Change(path="file.txt", before=fingerprint(path=root / "file.txt"), after=after)],
    )
    with pytest.raises(CowtreeError, match="corrupt object"):
        Installation.prepare(directory=tmp_path / "transaction", record=record, objects=objects)
    assert (root / "file.txt").read_bytes() == b"original\n"
    assert not (tmp_path / "transaction").exists()
