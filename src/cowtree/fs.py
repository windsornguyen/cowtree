"""Probe native copy-on-write support in an existing directory."""

from __future__ import annotations

from pathlib import Path
import platform
import tempfile

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.native import clone_regular_file
from cowtree.types import CloneTool, DoctorReport, FilesystemKind


def doctor(path: Path) -> DoctorReport:
    """Probe clone availability and write isolation without modifying existing files."""
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
        report = clone_probe_report(
            path=resolved, filesystem=FilesystemKind.CLONEFILE, clone_tool=CloneTool.MACOS_CLONEFILE
        )
        return report

    if system == "Linux":
        report = clone_probe_report(
            path=resolved, filesystem=FilesystemKind.REFLINK, clone_tool=CloneTool.LINUX_FICLONE
        )
        return report

    if system == "Windows":
        report = clone_probe_report(
            path=resolved,
            filesystem=FilesystemKind.BLOCK_CLONE,
            clone_tool=CloneTool.WINDOWS_EXTENTS,
        )
        return report

    report = DoctorReport(
        path=resolved,
        filesystem=FilesystemKind.UNSUPPORTED,
        reason=f"{system} is unsupported",
    )
    return report


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
            changed = b"changed\n"
            dst.write_bytes(changed)
            if src.read_bytes() != payload:
                raise CowtreeError(
                    code=CowtreeErrorCode.COMMAND_FAILED, message="CoW probe modified the source"
                )
            src.write_bytes(b"source changed\n")
            if dst.read_bytes() != changed:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "CoW probe modified the clone")
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
