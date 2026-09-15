"""Probe native copy-on-write support in an existing directory."""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
import platform
import plistlib
import tempfile
from typing import TYPE_CHECKING
from xml.parsers.expat import ExpatError

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.native import clone_regular_file
from cowtree.types import CloneTool, CommandResult, DoctorReport, FilesystemKind


if TYPE_CHECKING:
    import pytest


def doctor(path: Path, runner: CommandRunner | None = None) -> DoctorReport:
    """Probe clone availability and write isolation without modifying existing files."""
    runner = runner if runner is not None else CommandRunner()
    try:
        resolved = path.resolve(strict=True)
        if not resolved.is_dir():
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, f"doctor requires an existing directory: {path}"
            )
    except (FileNotFoundError, NotADirectoryError, RuntimeError, ValueError) as error:
        raise CowtreeError(
            CowtreeErrorCode.INVALID_ARGUMENTS, f"invalid doctor directory {path}: {error}"
        ) from error
    except OSError as error:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, f"cannot inspect {path}: {error}"
        ) from error
    system = platform.system()
    if system == "Darwin":
        filesystem = darwin_filesystem_type(path=resolved, runner=runner)
        if filesystem == "apfs":
            report = clone_probe_report(
                path=resolved, filesystem=FilesystemKind.APFS, clone_tool=CloneTool.MACOS_CLONEFILE
            )
            return report
        report = DoctorReport(
            path=resolved, filesystem=FilesystemKind.UNSUPPORTED, reason=f"{filesystem} is not APFS"
        )
        return report

    if system == "Linux":
        report = clone_probe_report(
            path=resolved, filesystem=FilesystemKind.REFLINK, clone_tool=CloneTool.LINUX_FICLONE
        )
        return report

    report = DoctorReport(
        path=resolved,
        filesystem=FilesystemKind.UNSUPPORTED,
        reason=f"{system} is unsupported",
    )
    return report


def darwin_filesystem_type(path: Path, runner: CommandRunner) -> str:
    """Read the filesystem type reported by diskutil for a directory's device."""
    df = runner.run(argv=["df", "-P", str(path)]).stdout.splitlines()
    if len(df) < 2:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, f"df did not return a filesystem for {path}"
        )

    device = df[1].split()[0]
    data = runner.run(argv=["diskutil", "info", "-plist", device]).stdout.encode()
    try:
        plist = plistlib.loads(data)
    except (ValueError, ExpatError) as error:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, f"invalid diskutil output for {path}: {error}"
        ) from error
    if not isinstance(plist, dict):
        raise CowtreeError(
            code=CowtreeErrorCode.COMMAND_FAILED, message="diskutil did not return a dictionary"
        )
    filesystem = plist.get("FilesystemType")
    if not isinstance(filesystem, str):
        raise CowtreeError(
            code=CowtreeErrorCode.COMMAND_FAILED, message="diskutil omitted the filesystem type"
        )
    if not filesystem:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, "diskutil returned an empty filesystem type"
        )
    return filesystem


def clone_probe_report(
    path: Path, filesystem: FilesystemKind, clone_tool: CloneTool
) -> DoctorReport:
    """Verify a disposable clone's bytes, inode identity, and write isolation."""
    try:
        with tempfile.TemporaryDirectory(dir=path) as tmp:
            src = Path(tmp) / "src"
            dst = Path(tmp) / "dst"
            payload = b"cowtree probe\n"
            src.write_bytes(payload)
            clone_regular_file(source=src, target=dst)
            if dst.read_bytes() != payload:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "CoW probe changed file contents"
                )
            if src.stat().st_ino == dst.stat().st_ino:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "CoW probe reused the source inode"
                )
            dst.write_bytes(b"changed\n")
            if src.read_bytes() != payload:
                raise CowtreeError(
                    code=CowtreeErrorCode.COMMAND_FAILED, message="CoW probe modified the source"
                )
    except CowtreeError as error:
        if error.code != CowtreeErrorCode.COW_UNAVAILABLE:
            raise
        report = DoctorReport(
            path=path, filesystem=FilesystemKind.UNSUPPORTED, reason=error.message
        )
        return report
    except OSError as error:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, f"cannot probe CoW support in {path}: {error}"
        ) from error

    report = DoctorReport(path=path, filesystem=filesystem, clone_tool=clone_tool)
    return report


# --- Tests ---

from inline_tests import test  # noqa: E402


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
            assert cwd is None
            assert env is None
            assert check
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
            raise AssertionError(f"unexpected command: {argv}")

    def fake_clone(source: Path, target: Path) -> None:
        target.write_bytes(source.read_bytes())

    monkeypatch.setattr(platform, "system", lambda: "Darwin")
    monkeypatch.setattr("cowtree.native.clonefile", fake_clone)
    assert doctor(path=Path("."), runner=FakeRunner()).clone_tool == CloneTool.MACOS_CLONEFILE
