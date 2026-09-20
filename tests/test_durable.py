"""Group writeout keeps every file and namespace before one device-cache flush."""

import errno
import os
from pathlib import Path
import sys

import pytest

from cowtree import durable


def test_tree_writeouts_precede_the_single_device_flush(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    directory = tmp_path / "tree"
    directory.mkdir()
    first, second = directory / "one", directory / "two"
    first.write_bytes(b"one")
    second.write_bytes(b"two")
    calls: list[tuple[str, int]] = []
    fsync = os.fsync
    full = durable.fcntl.fcntl

    def writeout(descriptor: int) -> None:
        calls.append(("writeout", os.fstat(descriptor).st_ino))
        fsync(descriptor)

    def barrier(descriptor: int, command: int) -> int:
        calls.append(("full", os.fstat(descriptor).st_ino))
        return full(descriptor, command)

    monkeypatch.setattr(durable.os, "fsync", writeout)
    monkeypatch.setattr(durable.fcntl, "fcntl", barrier)
    durable.sync_tree(root=tmp_path, files=(first, second), directories=(directory,))
    expected = [("writeout", path.stat().st_ino) for path in (first, second, directory, tmp_path)]
    if sys.platform == "darwin":
        expected.append(("full", tmp_path.stat().st_ino))
    assert calls == expected


def test_group_rejects_a_symlink_payload_without_following_it(tmp_path: Path) -> None:
    target = tmp_path / "target"
    target.write_bytes(b"preserved")
    link = tmp_path / "link"
    link.symlink_to(target)
    with pytest.raises(OSError, match=r"(Too many levels|Symbolic link)"):
        durable.sync_tree(root=tmp_path, files=(link,), directories=())
    assert target.read_bytes() == b"preserved"


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS device-cache barrier")
def test_final_device_flush_failure_is_reported_and_all_descriptors_are_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "file"
    path.write_bytes(b"unacknowledged")
    descriptors: list[int] = []

    def fail(descriptor: int, command: int) -> int:
        assert command == 51
        descriptors.append(descriptor)
        raise OSError(errno.EIO, "injected device flush failure")

    monkeypatch.setattr(durable.fcntl, "fcntl", fail)
    with pytest.raises(OSError, match="injected device flush"):
        durable.sync_tree(root=tmp_path, files=(path,), directories=())
    assert len(descriptors) == 1
    with pytest.raises(OSError, match="Bad file descriptor") as failure:
        os.fstat(descriptors[0])
    assert failure.value.errno == errno.EBADF


def test_failed_file_writeout_does_not_attempt_a_final_barrier(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "file"
    path.write_bytes(b"unacknowledged")
    calls: list[int] = []

    def fail(descriptor: int) -> None:
        del descriptor
        raise OSError(errno.EIO, "injected writeout failure")

    def barrier(descriptor: int, command: int) -> int:
        del command
        calls.append(descriptor)
        return 0

    monkeypatch.setattr(durable.os, "fsync", fail)
    monkeypatch.setattr(durable.fcntl, "fcntl", barrier)
    with pytest.raises(OSError, match="injected writeout"):
        durable.sync_tree(root=tmp_path, files=(path,), directories=())
    assert calls == []
