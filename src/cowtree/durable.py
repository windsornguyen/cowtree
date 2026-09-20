"""Durable records and directory flushes for owned workspace metadata."""

from __future__ import annotations

from collections.abc import Iterable
import errno
import fcntl
import os
from pathlib import Path
import stat
import sys
import tempfile

from pydantic import BaseModel


def sync_file(path: Path) -> None:
    """Flush one regular file before making its name authoritative."""
    with path.open("rb") as stream:
        os.fsync(stream.fileno())
        if sys.platform == "darwin":
            fcntl.fcntl(stream.fileno(), 51)  # Darwin F_FULLFSYNC.


def sync_directory(path: Path) -> None:
    """Flush directory entries after creation, replacement, or removal."""
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def sync_tree(root: Path, files: Iterable[Path], directories: Iterable[Path]) -> None:
    """Flush a quiescent tree on one filesystem before acknowledging its record.

    Write every file and directory to the device before requesting one macOS
    device-cache flush. Directories must include every changed parent below root,
    ordered from children to parents; root is flushed last. Any failed writeout
    aborts the group without acknowledgement. Linux fsync retains its normal
    device-cache semantics for each file and directory.
    """
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        device = os.fstat(descriptor).st_dev
        for paths, directory in ((files, False), (directories, True)):
            for path in paths:
                flags = os.O_RDONLY | os.O_NOFOLLOW
                if directory:
                    flags |= os.O_DIRECTORY
                entry = os.open(path, flags)
                try:
                    metadata = os.fstat(entry)
                    if metadata.st_dev != device:
                        raise OSError(errno.EXDEV, "flush group crosses filesystems", str(path))
                    if not directory and not stat.S_ISREG(metadata.st_mode):
                        raise OSError(
                            errno.EINVAL, "flush group requires a regular file", str(path)
                        )
                    os.fsync(entry)
                finally:
                    os.close(entry)
        os.fsync(descriptor)
        if sys.platform == "darwin":
            fcntl.fcntl(descriptor, 51)  # Darwin F_FULLFSYNC covers the completed writeouts.
    finally:
        os.close(descriptor)


def write_record(path: Path, record: BaseModel) -> None:
    """Replace an owned record only after its complete bytes have been flushed."""
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".pending-", delete=False) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(record.model_dump_json().encode())
            stream.flush()
            os.fsync(stream.fileno())
        except BaseException:
            temporary.unlink()
            raise
    try:
        sync_file(path=temporary)
        os.replace(temporary, path)
        sync_directory(path=path.parent)
    finally:
        temporary.unlink(missing_ok=True)
