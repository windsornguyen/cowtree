"""Clone regular files through clonefile(2) or ioctl_ficlone(2).

The syscall boundary owns destination creation, metadata, and failure cleanup.
Callers coordinate source writers and keep parent directories stable.
"""

from __future__ import annotations

import ctypes
import errno
import fcntl
from functools import cache
import os
from pathlib import Path
import platform
import stat

from cowtree.errors import CowtreeError, CowtreeErrorCode


FICLONE = 0x40049409


@cache
def system_library() -> ctypes.CDLL:
    """Reuse the system library while ctypes retains errno per thread."""
    library = ctypes.CDLL(None, use_errno=True)
    return library


def clone_regular_file(source: Path, target: Path) -> None:
    """Create an independent native clone at an absent destination."""
    try:
        if not stat.S_ISREG(source.lstat().st_mode):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, f"source is not a regular file: {source}"
            )
        if os.path.lexists(target):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, f"destination already exists: {target}"
            )

        system = platform.system()
        if system == "Darwin":
            clonefile(source=source, target=target)
            return
        if system == "Linux":
            reflink(source=source, target=target)
            return
        raise CowtreeError(
            code=CowtreeErrorCode.COW_UNAVAILABLE, message=f"{system} is unsupported"
        )
    except OSError as error:
        raise clone_error(target=target, error=error) from error


def clonefile(source: Path, target: Path) -> None:
    """Call macOS clonefile(2), retaining errno on failure."""
    libc = system_library()
    clone = libc.clonefile
    clone.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint32)
    clone.restype = ctypes.c_int
    result = clone(os.fsencode(source), os.fsencode(target), 0)
    if result == 0:
        return
    err = ctypes.get_errno()
    raise OSError(err, os.strerror(err), target)


def reflink(source: Path, target: Path) -> None:
    """Clone with Linux FICLONE and clean up the owned destination on failure."""
    with source.open("rb") as src, target.open("xb") as dst:
        try:
            metadata = os.fstat(src.fileno())
            fcntl.ioctl(dst.fileno(), FICLONE, src.fileno())
            os.fchmod(dst.fileno(), stat.S_IMODE(metadata.st_mode))
            os.utime(dst.fileno(), ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        except BaseException as original:
            try:
                target.unlink()
            except OSError as cleanup:
                raise CowtreeError(
                    code=CowtreeErrorCode.CLEANUP_FAILED,
                    message=f"cannot remove failed clone {target}: {cleanup}; "
                    f"clone failed: {original}",
                ) from original
            raise


def clone_error(target: Path, error: OSError) -> CowtreeError:
    """Classify native failures without replacing the operating-system cause."""
    if error.errno in (errno.ENOTSUP, errno.EOPNOTSUPP, errno.ENOTTY, errno.EXDEV):
        code = CowtreeErrorCode.COW_UNAVAILABLE
    elif error.errno in (errno.EEXIST, errno.ENOENT, errno.ENOTDIR):
        code = CowtreeErrorCode.INVALID_ARGUMENTS
    else:
        code = CowtreeErrorCode.COMMAND_FAILED
    result = CowtreeError(code=code, message=f"CoW clone failed for {target}: {error}")
    return result
