"""Clone ReFS file extents without an ordinary-copy path.

Only Windows loads this adapter. Volume capabilities, cluster alignment, and
integrity settings are checked before cloning. The owned destination is removed
after handles close if any operation fails. Callers keep source writers quiescent.
"""

import ctypes
from ctypes import wintypes
from dataclasses import dataclass
from enum import IntEnum
import os
from pathlib import Path
import stat
import sys

from cowtree.errors import CowtreeError, CowtreeErrorCode


if sys.platform == "win32":
    from ctypes import WinDLL
    import msvcrt


class Control(IntEnum):
    """Control codes from winioctl.h, sent only to open file handles."""

    DUPLICATE_EXTENTS = 0x00098344
    GET_INTEGRITY = 0x0009027C
    SET_INTEGRITY = 0x0009C280
    SET_SPARSE = 0x000900C4


class Extents(ctypes.Structure):
    _fields_ = [
        ("source", wintypes.HANDLE),
        ("source_offset", ctypes.c_int64),
        ("target_offset", ctypes.c_int64),
        ("length", ctypes.c_int64),
    ]


class Integrity(ctypes.Structure):
    _fields_ = [
        ("algorithm", wintypes.WORD),
        ("reserved", wintypes.WORD),
        ("flags", wintypes.DWORD),
        ("chunk", wintypes.DWORD),
        ("cluster", wintypes.DWORD),
    ]


class IntegritySetting(ctypes.Structure):
    _fields_ = [
        ("algorithm", wintypes.WORD),
        ("reserved", wintypes.WORD),
        ("flags", wintypes.DWORD),
    ]


@dataclass(frozen=True)
class Volume:
    """Observed volume identity and native extent granularity."""

    serial: int
    cluster: int


class Kernel:
    """Own typed Win32 function signatures and preserve native error codes."""

    dll: ctypes.CDLL

    def __init__(self) -> None:
        if sys.platform != "win32":
            raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "Windows backend requires Windows")
        self.dll = WinDLL("kernel32", use_last_error=True)
        self.dll.GetVolumePathNameW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, wintypes.DWORD]
        self.dll.GetVolumePathNameW.restype = wintypes.BOOL
        self.dll.GetVolumeInformationW.argtypes = [
            wintypes.LPCWSTR,
            wintypes.LPWSTR,
            wintypes.DWORD,
            ctypes.POINTER(wintypes.DWORD),
            ctypes.POINTER(wintypes.DWORD),
            ctypes.POINTER(wintypes.DWORD),
            wintypes.LPWSTR,
            wintypes.DWORD,
        ]
        self.dll.GetVolumeInformationW.restype = wintypes.BOOL
        self.dll.GetDiskFreeSpaceW.argtypes = [
            wintypes.LPCWSTR,
            *([ctypes.POINTER(wintypes.DWORD)] * 4),
        ]
        self.dll.GetDiskFreeSpaceW.restype = wintypes.BOOL
        self.dll.DeviceIoControl.argtypes = [
            wintypes.HANDLE,
            wintypes.DWORD,
            wintypes.LPVOID,
            wintypes.DWORD,
            wintypes.LPVOID,
            wintypes.DWORD,
            ctypes.POINTER(wintypes.DWORD),
            wintypes.LPVOID,
        ]
        self.dll.DeviceIoControl.restype = wintypes.BOOL

    def failure(self, operation: str) -> None:
        if sys.platform != "win32":
            raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "Windows backend requires Windows")
        code = ctypes.get_last_error()
        category = CowtreeErrorCode.COMMAND_FAILED
        if code in (1, 17, 50):  # Invalid function, cross-device operation, unsupported capability.
            category = CowtreeErrorCode.COW_UNAVAILABLE
        raise CowtreeError(category, f"{operation} failed with Windows error {code}")

    def volume(self, path: Path) -> Volume:
        root = ctypes.create_unicode_buffer(32768)
        if not self.dll.GetVolumePathNameW(str(path.resolve()), root, len(root)):
            self.failure(operation="GetVolumePathNameW")
        serial, flags = wintypes.DWORD(), wintypes.DWORD()
        if not self.dll.GetVolumeInformationW(
            root, None, 0, ctypes.byref(serial), None, ctypes.byref(flags), None, 0
        ):
            self.failure(operation="GetVolumeInformationW")
        if not flags.value & 0x08000000:  # FILE_SUPPORTS_BLOCK_REFCOUNTING.
            raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "volume lacks block refcounting")
        sectors, size, free, total = (wintypes.DWORD() for _ in range(4))
        if not self.dll.GetDiskFreeSpaceW(
            root, ctypes.byref(sectors), ctypes.byref(size), ctypes.byref(free), ctypes.byref(total)
        ):
            self.failure(operation="GetDiskFreeSpaceW")
        cluster = sectors.value * size.value
        if cluster <= 0 or cluster & (cluster - 1):
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "invalid volume cluster size")
        result = Volume(serial=serial.value, cluster=cluster)
        return result

    def control(
        self,
        handle: int,
        code: Control,
        data: ctypes.Structure | None = None,
        output: ctypes.Structure | None = None,
    ) -> None:
        count = wintypes.DWORD()
        src = None if data is None else ctypes.byref(data)
        dst = None if output is None else ctypes.byref(output)
        if not self.dll.DeviceIoControl(
            handle,
            int(code),
            src,
            0 if data is None else ctypes.sizeof(data),
            dst,
            0 if output is None else ctypes.sizeof(output),
            ctypes.byref(count),
            None,
        ):
            self.failure(operation=code.name)

    def clone(self, source: Path, target: Path) -> None:
        if sys.platform != "win32":
            raise CowtreeError(CowtreeErrorCode.COW_UNAVAILABLE, "Windows backend requires Windows")
        volume = self.volume(path=source)
        destination = self.volume(path=target.parent)
        if volume != destination:
            raise CowtreeError(CowtreeErrorCode.DIFFERENT_FILESYSTEM, "clone volumes differ")
        owned = False
        try:
            with source.open("rb") as src, target.open("xb") as dst:
                owned = True
                source_handle = msvcrt.get_osfhandle(src.fileno())
                target_handle = msvcrt.get_osfhandle(dst.fileno())
                integrity = Integrity()
                self.control(handle=source_handle, code=Control.GET_INTEGRITY, output=integrity)
                setting = IntegritySetting(integrity.algorithm, 0, integrity.flags)
                self.control(handle=target_handle, code=Control.SET_INTEGRITY, data=setting)
                self.control(handle=target_handle, code=Control.SET_SPARSE)
                metadata = os.fstat(src.fileno())
                dst.truncate(metadata.st_size)
                self.extents(
                    source=source_handle,
                    target=target_handle,
                    size=metadata.st_size,
                    cluster=volume.cluster,
                )
            os.utime(target, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
            os.chmod(target, stat.S_IMODE(metadata.st_mode))
        except BaseException as original:
            if owned:
                try:
                    target.unlink()
                except OSError as cleanup:
                    raise CowtreeError(
                        CowtreeErrorCode.CLEANUP_FAILED,
                        f"failed clone remains at {target}: {cleanup}",
                    ) from original
            raise

    def extents(self, source: int, target: int, size: int, cluster: int) -> None:
        # Match ReFS's cluster-rounded EOF contract; each native request must be below 4 GiB.
        rounded = (size + cluster - 1) // cluster * cluster
        chunk = (1 << 32) - cluster
        for offset in range(0, rounded, chunk):
            data = Extents(source, offset, offset, min(chunk, rounded - offset))
            self.control(handle=target, code=Control.DUPLICATE_EXTENTS, data=data)
