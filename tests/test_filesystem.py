from __future__ import annotations

from collections.abc import Mapping
import errno
import mmap
import os
from pathlib import Path
import platform
import sys

import pytest

from cowtree import fs, native
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.types import CloneTool, CommandResult, FilesystemKind


@pytest.mark.parametrize("payload", [b"a\r\nb\rc\n", b"raw\xff\x80\x00bytes\r\n"])
def test_command_output_round_trips_bytes(payload: bytes) -> None:
    result = CommandRunner().run(
        argv=[
            sys.executable,
            "-c",
            "import os,sys; b=bytes.fromhex(sys.argv[1]); os.write(1,b); os.write(2,b)",
            payload.hex(),
        ]
    )
    assert result.stdout.encode("utf-8", "surrogateescape") == payload
    assert result.stderr.encode("utf-8", "surrogateescape") == payload


@pytest.mark.parametrize("missing_cwd", [False, True])
def test_process_start_errors_are_typed(tmp_path: Path, missing_cwd: bool) -> None:
    missing = tmp_path / "missing"
    argv = [sys.executable, "-c", "pass"] if missing_cwd else [str(missing)]
    with pytest.raises(CowtreeError) as caught:
        CommandRunner().run(argv=argv, cwd=str(missing) if missing_cwd else None)
    assert caught.value.code == CowtreeErrorCode.COMMAND_FAILED


@pytest.mark.parametrize("make_file", [False, True])
def test_doctor_requires_existing_directory(tmp_path: Path, make_file: bool) -> None:
    path = tmp_path / "path"
    if make_file:
        path.touch()
    with pytest.raises(CowtreeError) as caught:
        fs.doctor(path=path)
    assert caught.value.code == CowtreeErrorCode.INVALID_ARGUMENTS


@pytest.mark.parametrize("system", ["Darwin", "Linux"])
def test_doctor_uses_native_probe_without_disk_management(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, system: str
) -> None:
    def unavailable_command(
        self: CommandRunner,
        argv: list[str],
        *,
        cwd: str | None = None,
        env: Mapping[str, str] | None = None,
        check: bool = True,
    ) -> CommandResult:
        del self, argv, cwd, env, check
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "DiskManagement framework unavailable")

    def clone(source: Path, target: Path) -> None:
        target.write_bytes(source.read_bytes())

    monkeypatch.setattr(platform, "system", lambda: system)
    monkeypatch.setattr(CommandRunner, "run", unavailable_command)
    monkeypatch.setattr(native, "clonefile" if system == "Darwin" else "reflink", clone)
    report = fs.doctor(path=tmp_path)
    assert report.supported
    assert report.filesystem == (
        FilesystemKind.CLONEFILE if system == "Darwin" else FilesystemKind.REFLINK
    )
    assert report.clone_tool == (
        CloneTool.MACOS_CLONEFILE if system == "Darwin" else CloneTool.LINUX_FICLONE
    )
    assert list(tmp_path.iterdir()) == []


def test_doctor_probes_apfs_clone_support(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(platform, "system", lambda: "Darwin")

    def unavailable(source: Path, target: Path) -> None:  # noqa: ARG001
        raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "clone disabled")

    monkeypatch.setattr(native, "clonefile", unavailable)
    report = fs.doctor(path=tmp_path)
    assert not report.supported
    assert report.reason == "clone disabled"
    assert list(tmp_path.iterdir()) == []


def test_doctor_preserves_operational_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(platform, "system", lambda: "Darwin")

    def inaccessible(source: Path, target: Path) -> None:  # noqa: ARG001
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "permission denied")

    monkeypatch.setattr(native, "clonefile", inaccessible)
    with pytest.raises(CowtreeError) as caught:
        fs.doctor(path=tmp_path)
    assert caught.value.code == CowtreeErrorCode.COMMAND_FAILED
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize("kind", ["directory", "symlink"])
def test_clone_requires_regular_source(tmp_path: Path, kind: str) -> None:
    source = tmp_path / "source"
    if kind == "directory":
        source.mkdir()
    else:
        regular = tmp_path / "regular"
        regular.touch()
        source.symlink_to(regular)
    with pytest.raises(CowtreeError) as caught:
        fs.clone_regular_file(source=source, target=tmp_path / "target")
    assert caught.value.code == CowtreeErrorCode.INVALID_ARGUMENTS
    assert not (tmp_path / "target").exists()


@pytest.mark.parametrize("dangling", [False, True])
def test_clone_preserves_existing_destination(tmp_path: Path, dangling: bool) -> None:
    source = tmp_path / "source"
    target = tmp_path / "target"
    source.write_bytes(b"source")
    if dangling:
        target.symlink_to("missing")
    else:
        target.write_bytes(b"existing")
    with pytest.raises(CowtreeError) as caught:
        fs.clone_regular_file(source=source, target=target)
    assert caught.value.code == CowtreeErrorCode.INVALID_ARGUMENTS
    if dangling:
        assert os.readlink(target) == "missing"
    else:
        assert target.read_bytes() == b"existing"


def test_native_clone_preserves_os_error_source(tmp_path: Path) -> None:
    source = tmp_path / "source"
    target = tmp_path / "missing" / "target"
    source.write_bytes(b"source")
    with pytest.raises(CowtreeError) as caught:
        fs.clone_regular_file(source=source, target=target)
    assert caught.value.code == CowtreeErrorCode.INVALID_ARGUMENTS
    assert isinstance(caught.value.__cause__, OSError)
    assert caught.value.__cause__.errno == errno.ENOENT


@pytest.mark.parametrize("operation", ["append", "pwrite", "truncate", "mmap", "replace"])
@pytest.mark.parametrize("mutate_source", [False, True])
@pytest.mark.parametrize("name", ["plain", "line\nbreak\r", os.fsdecode(b"raw-\xff\x80")])
def test_native_clone_mutation_isolation(
    tmp_path: Path, operation: str, mutate_source: bool, name: str
) -> None:
    report = fs.doctor(path=tmp_path)
    if not report.supported:
        pytest.skip(report.reason or "CoW unavailable")
    source = tmp_path / name
    target = tmp_path / (name + "-clone")
    payload = bytes(range(256)) * 64
    try:
        source.write_bytes(payload)
    except OSError as error:
        if error.errno == errno.EILSEQ or (sys.platform == "darwin" and error.errno == errno.EPERM):
            pytest.skip("filesystem requires UTF-8 filenames")
        raise
    source.chmod(0o751)
    os.utime(source, ns=(1_700_000_000_000_000_000, 1_700_000_100_000_000_000))
    original = source.stat()
    fs.clone_regular_file(source=source, target=target)
    cloned = target.stat()
    assert cloned.st_ino != original.st_ino
    assert cloned.st_mode == original.st_mode
    assert cloned.st_mtime_ns == original.st_mtime_ns
    assert target.read_bytes() == payload
    changed, stable = (source, target) if mutate_source else (target, source)
    if operation == "replace":
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"replacement")
        replacement.replace(changed)
    else:
        with changed.open("r+b") as stream:
            if operation == "append":
                stream.seek(0, os.SEEK_END)
                stream.write(b"append")
            elif operation == "pwrite":
                os.pwrite(stream.fileno(), b"pwrite", 8192)
            elif operation == "truncate":
                stream.truncate(123)
            elif operation == "mmap":
                with mmap.mmap(stream.fileno(), 0) as mapped:
                    mapped[4096:4100] = b"mmap"
                    mapped.flush()
            else:
                raise AssertionError(operation)
    assert changed.read_bytes() != payload
    assert stable.read_bytes() == payload


def test_linux_reflink_cleans_failed_owned_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import fcntl

    source = tmp_path / "source"
    target = tmp_path / "target"
    source.write_bytes(b"source")
    monkeypatch.setattr(platform, "system", lambda: "Linux")

    def unsupported(_target: int, _operation: int, _source: int) -> int:
        raise OSError(errno.EOPNOTSUPP, "clone unavailable")

    monkeypatch.setattr(fcntl, "ioctl", unsupported)
    with pytest.raises(CowtreeError) as caught:
        fs.clone_regular_file(source=source, target=target)
    assert caught.value.code == CowtreeErrorCode.COW_UNAVAILABLE
    assert not target.exists()
    assert source.read_bytes() == b"source"


def test_linux_reflink_does_not_own_racing_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    target = tmp_path / "target"
    source.write_bytes(b"source")
    native_reflink = native.reflink

    def raced_clone(source: Path, target: Path) -> None:
        target.write_bytes(b"concurrent owner")
        native_reflink(source=source, target=target)

    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(native, "reflink", raced_clone)
    with pytest.raises(CowtreeError) as caught:
        fs.clone_regular_file(source=source, target=target)
    assert caught.value.code == CowtreeErrorCode.INVALID_ARGUMENTS
    assert target.read_bytes() == b"concurrent owner"


@pytest.mark.parametrize(
    "failure", [errno.EOPNOTSUPP, errno.ENOTTY, errno.EXDEV, errno.EIO, errno.EACCES]
)
def test_linux_probe_distinguishes_unsupported_from_failed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: int
) -> None:
    import fcntl

    def failing_clone(target: int, _operation: int, _source: int) -> int:
        os.write(target, b"partial clone")
        raise OSError(failure, os.strerror(failure))

    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(fcntl, "ioctl", failing_clone)
    if failure in (errno.EOPNOTSUPP, errno.ENOTTY, errno.EXDEV):
        report = fs.doctor(path=tmp_path)
        assert not report.supported
    else:
        with pytest.raises(CowtreeError) as caught:
            fs.doctor(path=tmp_path)
        assert caught.value.code == CowtreeErrorCode.COMMAND_FAILED
    assert list(tmp_path.iterdir()) == []


def test_linux_reflink_preserves_metadata(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import fcntl

    source = tmp_path / "source"
    target = tmp_path / "target"
    source.write_bytes(b"source")
    source.chmod(0o751)
    os.utime(source, ns=(1_700_000_000_000_000_000, 1_700_000_100_000_000_000))
    original = source.stat()

    def simulated_ioctl(target: int, operation: int, source: int) -> int:
        assert operation == 0x40049409
        os.write(target, os.read(source, 4096))
        return 0

    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(fcntl, "ioctl", simulated_ioctl)
    fs.clone_regular_file(source=source, target=target)
    cloned = target.stat()
    assert cloned.st_ino != original.st_ino
    assert cloned.st_mode == original.st_mode
    assert cloned.st_mtime_ns == original.st_mtime_ns
    assert cloned.st_atime_ns == original.st_atime_ns
    assert target.read_bytes() == b"source"


def test_linux_reflink_removes_clone_after_metadata_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import fcntl

    source = tmp_path / "source"
    target = tmp_path / "target"
    source.write_bytes(b"source")

    def simulated_ioctl(target: int, _operation: int, source: int) -> int:
        os.write(target, os.read(source, 4096))
        return 0

    def metadata_failure(_fd: int, _mode: int) -> None:
        raise OSError(errno.EPERM, "cannot set mode")

    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(fcntl, "ioctl", simulated_ioctl)
    monkeypatch.setattr(os, "fchmod", metadata_failure)
    with pytest.raises(CowtreeError) as caught:
        fs.clone_regular_file(source=source, target=target)
    assert caught.value.code == CowtreeErrorCode.COMMAND_FAILED
    assert not target.exists()
    assert source.read_bytes() == b"source"


def test_linux_reflink_reports_failed_cleanup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import fcntl

    source = tmp_path / "source"
    target = tmp_path / "target"
    source.write_bytes(b"source")
    failure = OSError(errno.EIO, "clone interrupted")

    def failed_clone(_target: int, _operation: int, _source: int) -> int:
        raise failure

    def failed_cleanup(self: Path, missing_ok: bool = False) -> None:  # noqa: ARG001
        raise OSError(errno.EACCES, "cleanup blocked", str(self))

    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(fcntl, "ioctl", failed_clone)
    monkeypatch.setattr(Path, "unlink", failed_cleanup)
    with pytest.raises(CowtreeError) as caught:
        fs.clone_regular_file(source=source, target=target)
    assert caught.value.code == CowtreeErrorCode.CLEANUP_FAILED
    assert caught.value.__cause__ is failure
    assert "cleanup blocked" in caught.value.message
    assert str(target) in caught.value.message
    assert target.exists()


def test_command_rejects_nul_arguments() -> None:
    with pytest.raises(CowtreeError) as caught:
        CommandRunner().run(argv=[sys.executable, "bad\x00argument"])
    assert caught.value.code == CowtreeErrorCode.INVALID_ARGUMENTS


@pytest.mark.parametrize("size", [0, 1, 4095, 4096, 4097, 2 * 1024 * 1024])
def test_native_clone_boundary_sizes(tmp_path: Path, size: int) -> None:
    report = fs.doctor(path=tmp_path)
    if not report.supported:
        pytest.skip(report.reason or "CoW unavailable")
    source = tmp_path / "source"
    target = tmp_path / "target"
    payload = (bytes(range(256)) * ((size + 255) // 256))[:size]
    source.write_bytes(payload)
    fs.clone_regular_file(source=source, target=target)
    assert target.read_bytes() == payload
    assert source.read_bytes() == payload
    assert target.stat().st_ino != source.stat().st_ino


def test_native_clone_sparse_file(tmp_path: Path) -> None:
    report = fs.doctor(path=tmp_path)
    if not report.supported:
        pytest.skip(report.reason or "CoW unavailable")
    source = tmp_path / "source"
    target = tmp_path / "target"
    with source.open("wb") as stream:
        stream.truncate(8 * 1024 * 1024)
        stream.seek(7 * 1024 * 1024)
        stream.write(b"sparse")
    fs.clone_regular_file(source=source, target=target)
    assert target.stat().st_size == source.stat().st_size
    assert target.stat().st_blocks <= source.stat().st_blocks
    assert target.read_bytes() == source.read_bytes()


@pytest.mark.parametrize("seed", [0, 17, 999])
def test_native_clone_fanout_matches_byte_model(tmp_path: Path, seed: int) -> None:
    import random

    report = fs.doctor(path=tmp_path)
    if not report.supported:
        pytest.skip(report.reason or "CoW unavailable")
    source = tmp_path / "source"
    payload = bytes(range(256)) * 64
    source.write_bytes(payload)
    paths = [tmp_path / f"clone-{index}" for index in range(8)]
    expected = [payload for _ in paths]
    for path in paths:
        fs.clone_regular_file(source=source, target=path)
    randomizer = random.Random(seed)  # noqa: S311
    for step in range(200):
        index = randomizer.randrange(len(paths))
        path = paths[index]
        operation = randomizer.randrange(3)
        data = randomizer.randbytes(randomizer.randrange(1, 256))
        with path.open("r+b") as stream:
            if operation == 0:
                offset = randomizer.randrange(32768)
                os.pwrite(stream.fileno(), data, offset)
                current = expected[index].ljust(offset, b"\0")
                expected[index] = current[:offset] + data + current[offset + len(data) :]
            elif operation == 1:
                length = randomizer.randrange(32768)
                stream.truncate(length)
                expected[index] = expected[index][:length].ljust(length, b"\0")
            else:
                stream.seek(0, os.SEEK_END)
                stream.write(data)
                expected[index] += data
        assert source.read_bytes() == payload, (seed, step)
        for candidate, model in zip(paths, expected, strict=True):
            assert candidate.read_bytes() == model, (seed, step, candidate)
