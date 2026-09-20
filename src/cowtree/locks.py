"""Hold the standalone repository lock until the current operation returns.

POSIX uses flock; Windows uses a blocking LockFileEx range. These locks serialize
live standalone operations. They do not provide managed child-process ownership
or recovery after an unhandled process crash.
"""

from collections.abc import Iterator
from contextlib import contextmanager
import ctypes
from ctypes import wintypes
import sys

from cowtree.errors import CowtreeError, CowtreeErrorCode


@contextmanager
def exclusive(descriptor: int) -> Iterator[None]:
    """Release the same native lock before its owning descriptor closes."""
    if sys.platform == "win32":
        lock = WindowsLock(descriptor=descriptor)
        lock.acquire()
        try:
            yield
        finally:
            lock.release()
        return
    import fcntl

    fcntl.flock(descriptor, fcntl.LOCK_EX)
    try:
        yield
    finally:
        fcntl.flock(descriptor, fcntl.LOCK_UN)


class Overlapped(ctypes.Structure):
    """Keep the native request alive through both lock and unlock calls."""

    _fields_ = [
        ("internal", ctypes.c_size_t),
        ("internal_high", ctypes.c_size_t),
        ("offset", wintypes.DWORD),
        ("offset_high", wintypes.DWORD),
        ("event", wintypes.HANDLE),
    ]


class WindowsLock:
    """Lock byte zero synchronously, including when the lock file is empty."""

    def __init__(self, descriptor: int) -> None:
        if sys.platform != "win32":
            raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "Windows lock requires Windows")
        import msvcrt

        from cowtree.windows import Kernel

        self.kernel = Kernel()
        self.handle = msvcrt.get_osfhandle(descriptor)
        self.request = Overlapped()
        self.kernel.dll.LockFileEx.argtypes = [
            wintypes.HANDLE,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.DWORD,
            ctypes.POINTER(Overlapped),
        ]
        self.kernel.dll.LockFileEx.restype = wintypes.BOOL
        self.kernel.dll.UnlockFileEx.argtypes = [
            wintypes.HANDLE,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.DWORD,
            ctypes.POINTER(Overlapped),
        ]
        self.kernel.dll.UnlockFileEx.restype = wintypes.BOOL

    def acquire(self) -> None:
        if sys.platform != "win32":
            raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "Windows lock requires Windows")
        # LOCKFILE_EXCLUSIVE_LOCK without FAIL_IMMEDIATELY waits in the kernel.
        if not self.kernel.dll.LockFileEx(self.handle, 2, 0, 1, 0, ctypes.byref(self.request)):
            self.kernel.failure(operation="LockFileEx")

    def release(self) -> None:
        if sys.platform != "win32":
            raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "Windows lock requires Windows")
        if not self.kernel.dll.UnlockFileEx(self.handle, 0, 1, 0, ctypes.byref(self.request)):
            self.kernel.failure(operation="UnlockFileEx")
