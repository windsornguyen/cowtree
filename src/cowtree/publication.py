"""Publish a completed local directory without replacing an existing pathname."""

import ctypes
import errno
import os
from pathlib import Path
import platform

from cowtree.durable import sync_directory
from cowtree.errors import CowtreeError, CowtreeErrorCode


def publish_directory(source: Path, target: Path) -> None:
    """Atomically claim an absent destination with the platform's exclusive rename."""
    if source.is_symlink() or not source.is_dir():
        raise CowtreeError(
            CowtreeErrorCode.INVALID_ARGUMENTS, "publication source is not a directory"
        )
    libc = ctypes.CDLL(None, use_errno=True)
    system = platform.system()
    try:
        if system == "Darwin":
            rename = libc.renamex_np
            rename.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
            rename.restype = ctypes.c_int
            result = rename(os.fsencode(source), os.fsencode(target), 4)  # RENAME_EXCL.
        elif system == "Linux":
            rename = libc.renameat2
            rename.argtypes = (
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_uint,
            )
            rename.restype = ctypes.c_int
            result = rename(
                -100, os.fsencode(source), -100, os.fsencode(target), 1
            )  # AT_FDCWD, RENAME_NOREPLACE.
        else:
            raise CowtreeError(
                CowtreeErrorCode.COW_UNAVAILABLE, f"exclusive rename unavailable on {system}"
            )
    except AttributeError as error:
        raise CowtreeError(
            CowtreeErrorCode.COW_UNAVAILABLE, "exclusive rename syscall is unavailable"
        ) from error
    if result != 0:
        code = ctypes.get_errno()
        error = OSError(code, os.strerror(code), target)
        category = (
            CowtreeErrorCode.INVALID_ARGUMENTS
            if code in (errno.EEXIST, errno.ENOTEMPTY)
            else CowtreeErrorCode.COMMAND_FAILED
        )
        raise CowtreeError(category, f"directory publication failed: {error}") from error
    sync_directory(path=target.parent)
    if source.parent != target.parent:
        sync_directory(path=source.parent)
