from __future__ import annotations

from collections.abc import Mapping
import ctypes
import errno
import os
from pathlib import Path
import platform
import plistlib
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
    resolved = path.resolve()
    if platform.system() == "Darwin":
        filesystem = darwin_filesystem_type(resolved, runner)
        if filesystem == "apfs":
            report = DoctorReport(
                path=resolved,
                filesystem=FilesystemKind.APFS,
                clone_tool=CloneTool.MACOS_CLONEFILE,
            )
            return report
        report = DoctorReport(path=resolved, filesystem=FilesystemKind.UNSUPPORTED, reason=f"{filesystem} is not APFS")
        return report

    if platform.system() == "Linux":
        report = linux_reflink_report(resolved, runner)
        return report

    report = DoctorReport(
        path=resolved,
        filesystem=FilesystemKind.UNSUPPORTED,
        reason=f"{platform.system()} is unsupported",
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


def clone_regular_file(source: Path, target: Path, runner: CommandRunner | None = None) -> None:
    runner = runner or CommandRunner()
    if target.exists() or target.is_symlink():
        raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"destination already exists: {target}")

    if platform.system() == "Darwin":
        clonefile(source, target)
        return

    if platform.system() == "Linux":
        runner.run(["cp", "--reflink=always", "--preserve=mode,timestamps", str(source), str(target)])
        return

    raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, f"{platform.system()} is unsupported")


def clonefile(source: Path, target: Path) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    result = libc.clonefile(os.fsencode(source), os.fsencode(target), 0)
    if result == 0:
        return

    err = ctypes.get_errno()
    reason = os.strerror(err)
    if err in (errno.ENOTSUP, errno.EXDEV):
        raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, f"clonefile failed for {target}: {reason}") from None
    raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, f"clonefile failed for {target}: {reason}") from None


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


def linux_reflink_report(path: Path, runner: CommandRunner) -> DoctorReport:
    try:
        with tempfile.TemporaryDirectory(dir=path) as tmp:
            src = Path(tmp) / "src"
            dst = Path(tmp) / "dst"
            src.write_text("cowtree probe\n")
            runner.run(["cp", "--reflink=always", str(src), str(dst)])
    except CowtreeError as error:
        report = DoctorReport(path=path, filesystem=FilesystemKind.UNSUPPORTED, reason=error.message)
        return report

    report = DoctorReport(path=path, filesystem=FilesystemKind.REFLINK, clone_tool=CloneTool.GNU_CP_REFLINK)
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

    monkeypatch.setattr(platform, "system", lambda: "Darwin")
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
