"""Durable records and directory flushes for owned workspace metadata."""

from __future__ import annotations

import fcntl
import os
from pathlib import Path
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
