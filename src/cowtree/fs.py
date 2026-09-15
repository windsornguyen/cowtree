from __future__ import annotations

from collections.abc import Mapping
import ctypes
import errno
import fcntl
import os
from pathlib import Path
import platform
import plistlib
import stat
import tempfile
from typing import TYPE_CHECKING

from inline_tests import test

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.models import CloneTool, CommandResult, DoctorReport, FilesystemKind


if TYPE_CHECKING:
    import pytest


def doctor(path: Path, runner: CommandRunner | None = None) -> DoctorReport:
    runner = runner or CommandRunner()
    try:
        resolved = path.resolve(strict=True)
        if not resolved.is_dir():
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"doctor requires an existing directory: {path}")
    except (FileNotFoundError, NotADirectoryError, RuntimeError, ValueError) as error:
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid doctor directory {path}: {error}") from error
    except OSError as error:
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"cannot inspect {path}: {error}") from error
    system = platform.system()
    if system == "Darwin":
        filesystem = darwin_filesystem_type(resolved, runner)
        if filesystem == "apfs":
            report = clone_probe_report(resolved, FilesystemKind.APFS, CloneTool.MACOS_CLONEFILE)
            return report
        report = DoctorReport(path=resolved, filesystem=FilesystemKind.UNSUPPORTED, reason=f"{filesystem} is not APFS")
        return report

    if system == "Linux":
        report = clone_probe_report(resolved, FilesystemKind.REFLINK, CloneTool.LINUX_FICLONE)
        return report

    report = DoctorReport(
        path=resolved,
        filesystem=FilesystemKind.UNSUPPORTED,
        reason=f"{system} is unsupported",
    )
    return report


def ensure_same_filesystem(source: Path, target: Path) -> None:
    if source.stat().st_dev != target.stat().st_dev:
        raise CowtreeError(
            CowtreeErrorCode.DIFFERENT_FILESYSTEM,
            "source and target are on different filesystems; CoW clone is unavailable",
        )


def ensure_cow_supported(source: Path, target: Path, runner: CommandRunner | None = None) -> DoctorReport:
    ensure_same_filesystem(source, target)
    report = doctor(target, runner)
    if not report.supported:
        raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, report.reason or "CoW clone is unavailable")
    return report


def clone_regular_file(source: Path, target: Path) -> None:
    try:
        if not stat.S_ISREG(source.lstat().st_mode):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"source is not a regular file: {source}")
        if target.exists() or target.is_symlink():
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"destination already exists: {target}")

        system = platform.system()
        if system == "Darwin":
            clonefile(source, target)
            return
        if system == "Linux":
            reflink(source, target)
            return
        raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, f"{system} is unsupported")
    except OSError as error:
        raise clone_error(target, error) from error


def clone_error(target: Path, error: OSError) -> CowtreeError:
    if error.errno in (errno.ENOTSUP, errno.EOPNOTSUPP, errno.ENOTTY, errno.EXDEV):
        code = CowtreeErrorCode.COW_UNAVAILABLE
    elif error.errno in (errno.EEXIST, errno.ENOENT, errno.ENOTDIR):
        code = CowtreeErrorCode.INVALID_ARGUMENTS
    else:
        code = CowtreeErrorCode.COMMAND_FAILED
    result = CowtreeError(code, f"CoW clone failed for {target}: {error}")
    return result


def clonefile(source: Path, target: Path) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    result = libc.clonefile(os.fsencode(source), os.fsencode(target), 0)
    if result == 0:
        return
    err = ctypes.get_errno()
    raise clone_error(target, OSError(err, os.strerror(err)))


def reflink(source: Path, target: Path) -> None:
    with source.open("rb") as src, target.open("xb") as dst:
        try:
            metadata = os.fstat(src.fileno())
            fcntl.ioctl(dst.fileno(), 0x40049409, src.fileno())  # Linux FICLONE.
            os.fchmod(dst.fileno(), stat.S_IMODE(metadata.st_mode))
            os.utime(dst.fileno(), ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        except BaseException:
            target.unlink()
            raise


def darwin_filesystem_type(path: Path, runner: CommandRunner) -> str:
    df = runner.run(["df", "-P", str(path)]).stdout.splitlines()
    if len(df) < 2:
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"df did not return a filesystem for {path}")

    device = df[1].split()[0]
    data = runner.run(["diskutil", "info", "-plist", device]).stdout.encode()
    plist = plistlib.loads(data)
    filesystem = plist.get("FilesystemType")
    name = str(filesystem or "unknown")
    return name


def clone_probe_report(path: Path, filesystem: FilesystemKind, clone_tool: CloneTool) -> DoctorReport:
    try:
        with tempfile.TemporaryDirectory(dir=path) as tmp:
            src = Path(tmp) / "src"
            dst = Path(tmp) / "dst"
            payload = b"cowtree probe\n"
            src.write_bytes(payload)
            clone_regular_file(src, dst)
            if dst.read_bytes() != payload or src.stat().st_ino == dst.stat().st_ino:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "CoW probe did not create an independent file")
            dst.write_bytes(b"changed\n")
            if src.read_bytes() != payload:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "CoW probe modified the source")
    except CowtreeError as error:
        if error.code != CowtreeErrorCode.COW_UNAVAILABLE:
            raise
        report = DoctorReport(path=path, filesystem=FilesystemKind.UNSUPPORTED, reason=error.message)
        return report
    except OSError as error:
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"cannot probe CoW support in {path}: {error}") from error

    report = DoctorReport(path=path, filesystem=filesystem, clone_tool=clone_tool)
    return report


@test
def parses_apfs_from_diskutil_plist(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakeRunner(CommandRunner):
        def run(
            self,
            argv: list[str],
            *,
            cwd: str | None = None,
            env: Mapping[str, str] | None = None,
            check: bool = True,
        ) -> CommandResult:
            if argv[:2] == ["df", "-P"]:
                result = CommandResult(
                    argv=argv,
                    returncode=0,
                    stdout="Filesystem 512-blocks Mounted on\n/dev/disk3s5 1 /\n",
                )
                return result
            if argv[:3] == ["diskutil", "info", "-plist"]:
                payload = plistlib.dumps({"FilesystemType": "apfs"}).decode()
                result = CommandResult(argv=argv, returncode=0, stdout=payload)
                return result
            result = super().run(argv, cwd=cwd, env=env, check=check)
            return result

    def fake_clone(source: Path, target: Path) -> None:
        target.write_bytes(source.read_bytes())

    monkeypatch.setattr(platform, "system", lambda: "Darwin")
    monkeypatch.setattr("cowtree.fs.clonefile", fake_clone)
    assert doctor(Path("."), FakeRunner()).clone_tool == CloneTool.MACOS_CLONEFILE


@test
def refuses_cross_filesystem_clone(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import pytest

    source = tmp_path / "source"
    target = tmp_path / "target"
    source.mkdir()
    target.mkdir()

    class Stat:
        def __init__(self, dev: int) -> None:
            self.st_dev = dev

    monkeypatch.setattr(Path, "stat", lambda self: Stat(1 if self == source else 2))

    with pytest.raises(CowtreeError) as error:
        ensure_same_filesystem(source, target)
    assert error.value.code == CowtreeErrorCode.DIFFERENT_FILESYSTEM
