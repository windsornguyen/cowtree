"""Require real ReFS cloning and explicit NTFS rejection on Windows runners."""

import ctypes
import os
from pathlib import Path

import pytest

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.native import clone_regular_file
from cowtree.windows import Control, Kernel


@pytest.mark.parametrize("size", [0, 1, 4095, 4096, 4097, 65537])
def test_refs_clone_is_independent(tmp_path: Path, size: int) -> None:
    source, target = tmp_path / "source", tmp_path / "target"
    data = bytes(range(256)) * (size // 256) + bytes(range(size % 256))
    source.write_bytes(data)
    clone_regular_file(source=source, target=target)
    assert target.read_bytes() == data
    assert source.stat().st_ino != target.stat().st_ino
    target.write_bytes(b"destination")
    assert source.read_bytes() == data
    source.write_bytes(b"source")
    assert target.read_bytes() == b"destination"


def test_ntfs_rejects_without_destination() -> None:
    root = Path(os.environ["COWTREE_NTFS_TEST"])
    root.mkdir()
    source, target = root / "source", root / "target"
    source.write_bytes(b"unmodified")
    with pytest.raises(CowtreeError) as err:
        clone_regular_file(source=source, target=target)
    assert err.value.code is CowtreeErrorCode.COW_UNAVAILABLE
    assert source.read_bytes() == b"unmodified"
    assert not target.exists()


def test_failed_extents_remove_only_the_owned_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source, target = tmp_path / "source", tmp_path / "target"
    source.write_bytes(b"keep source")
    original = Kernel.control

    def fail(
        self: Kernel,
        handle: int,
        code: Control,
        data: ctypes.Structure | None = None,
        output: ctypes.Structure | None = None,
    ) -> None:
        if code is Control.DUPLICATE_EXTENTS:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "injected extent failure")
        original(self, handle=handle, code=code, data=data, output=output)

    monkeypatch.setattr(Kernel, "control", fail)
    with pytest.raises(CowtreeError, match="injected extent failure"):
        clone_regular_file(source=source, target=target)
    assert source.read_bytes() == b"keep source"
    assert not target.exists()
    target.write_bytes(b"keep destination")
    with pytest.raises(CowtreeError) as err:
        clone_regular_file(source=source, target=target)
    assert err.value.code is CowtreeErrorCode.INVALID_ARGUMENTS
    assert target.read_bytes() == b"keep destination"
