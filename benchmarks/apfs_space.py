"""Measure allocated space inside a private APFS sparse image.

Container allocation counts shared extents once and includes filesystem metadata.
The sparse image's host allocation is a separate footprint: released container
blocks can remain allocated in the image until compaction, which this module
never performs.

Every sample detaches and reattaches the image normally to flush deferred APFS
allocation. Callers must close all files and stop workload processes before
sampling, and exclude this checkpoint from operation timings.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import time
from types import TracebackType
from typing import Literal, cast

from benchmarks.schema import JSONValue, expect_int, expect_mapping, expect_str
from cowtree.exec import CommandRunner


GIB = 1024**3
HOST_RESERVE_BYTES = 80 * GIB


class ImageError(RuntimeError):
    """Report an invalid image configuration or measurement boundary."""


@dataclass(frozen=True)
class ContainerInfo:
    reference: str
    size_bytes: int
    free_bytes: int
    volume_capacity_in_use_bytes: int

    @property
    def used_bytes(self) -> int:
        used = self.size_bytes - self.free_bytes
        return used


@dataclass(frozen=True)
class SpaceSample:
    used_bytes: int
    free_bytes: int
    container_size_bytes: int
    volume_capacity_in_use_bytes: int
    volume_used_bytes: int
    f_blocks: int
    f_bfree: int
    f_bavail: int
    f_frsize: int
    samples_used_bytes: tuple[int, ...]
    spread_bytes: int
    settled: bool
    image_allocated_bytes: int


@dataclass
class ManagedImage:
    root: Path
    name: str
    capacity_gib: int
    io: CommandRunner
    container_reference: str | None = field(default=None, init=False)

    @property
    def path(self) -> Path:
        path = self.root / f"{self.name}-mount"
        return path

    @property
    def image_path(self) -> Path:
        path = self.root / f"{self.name}.sparseimage"
        return path

    @property
    def closed_image_bytes(self) -> int:
        """Read the retained image's host allocation after a successful detach."""
        if self.path.is_mount():
            raise ImageError(f"image is still mounted at {self.path}")
        allocated = self.image_path.stat().st_blocks * 512
        return allocated

    def prepare(self) -> None:
        if platform.system() != "Darwin":
            raise ImageError("private APFS images require macOS")
        if re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,63}", self.name) is None:
            raise ImageError("image name must be a simple name of 1 to 64 characters")
        if not 1 <= self.capacity_gib <= 64:
            raise ImageError("image capacity must be between 1 and 64 GiB")
        self.root = self.root.resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        if os.path.lexists(self.image_path):
            raise ImageError(f"refusing existing image {self.image_path}")
        if os.path.lexists(self.path):
            raise ImageError(f"refusing existing mountpoint {self.path}")
        free = shutil.disk_usage(self.root).free
        if free - self.capacity_gib * GIB < HOST_RESERVE_BYTES:
            raise ImageError("image capacity would leave less than 80 GiB free on its host")
        self.path.mkdir()

    def __enter__(self) -> ManagedImage:
        self.prepare()
        ready = False
        try:
            self.io.run(
                argv=[
                    "hdiutil",
                    "create",
                    "-type",
                    "SPARSE",
                    "-fs",
                    "Case-sensitive APFS",
                    "-size",
                    f"{self.capacity_gib}g",
                    "-volname",
                    f"cowtree-{self.name}",
                    "-nospotlight",
                    str(self.image_path),
                ]
            )
            self.attach()
            ready = True
        finally:
            if not ready:
                self.close()
        image = self
        return image

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> Literal[False]:
        self.close()
        suppress = False
        return suppress

    def close(self) -> None:
        """Detach only this image's mountpoint and retain its backing image."""
        if self.path.is_mount():
            os.sync()
            self.io.run(argv=["hdiutil", "detach", str(self.path)])
        if self.path.is_mount():
            raise ImageError(f"detach left image mounted at {self.path}; image retained")
        self.container_reference = None
        if self.path.exists():
            self.path.rmdir()

    def attach(self) -> None:
        """Attach this exact backing image at its originally reserved mountpoint."""
        if self.path.is_mount():
            raise ImageError(f"refusing occupied mountpoint {self.path}")
        self.path.mkdir(exist_ok=True)
        self.io.run(
            argv=[
                "hdiutil",
                "attach",
                "-plist",
                "-nobrowse",
                "-mountpoint",
                str(self.path),
                str(self.image_path),
            ]
        )
        info = self.read_container()
        self.container_reference = info.reference

    def checkpoint(self) -> None:
        """Flush deferred allocation through a normal detach and reattach."""
        if self.container_reference is None:
            raise ImageError(f"image has no owned mounted container at {self.path}")
        self.read_container()
        os.sync()
        self.io.run(argv=["hdiutil", "detach", str(self.path)])
        self.container_reference = None
        self.attach()

    def read_container(self) -> ContainerInfo:
        if not self.path.is_mount():
            raise ImageError(f"expected mounted image at {self.path}")
        res = self.io.run(argv=["diskutil", "info", "-plist", str(self.path)])
        raw = expect_mapping(cast(JSONValue, plistlib.loads(res.stdout.encode("utf-8"))))
        if expect_str(raw["MountPoint"]) != str(self.path):
            raise ImageError(f"diskutil reported a different mountpoint for {self.path}")
        if expect_str(raw["FilesystemType"]) != "apfs":
            raise ImageError(f"expected APFS at {self.path}")
        if expect_str(raw["FilesystemName"]) != "Case-sensitive APFS":
            raise ImageError(f"expected case-sensitive APFS at {self.path}")
        if raw["APFSSnapshot"] is not False:
            raise ImageError(f"refusing APFS snapshot at {self.path}")
        reference = expect_str(raw["APFSContainerReference"])
        if self.container_reference is not None and reference != self.container_reference:
            raise ImageError(f"APFS container changed for {self.path}")
        info = ContainerInfo(
            reference=reference,
            size_bytes=expect_int(raw["APFSContainerSize"]),
            free_bytes=expect_int(raw["APFSContainerFree"]),
            volume_capacity_in_use_bytes=expect_int(raw["CapacityInUse"]),
        )
        if not 0 <= info.free_bytes <= info.size_bytes:
            raise ImageError(f"invalid APFS capacity counters for {self.path}")
        return info

    def sample(self) -> SpaceSample:
        """Checkpoint the image, then collect a stable allocation measurement."""
        self.checkpoint()
        readings: list[int] = []
        settled = False
        for _ in range(12):
            info = self.read_container()
            readings.append(info.used_bytes)
            settled = len(readings) >= 3 and len(set(readings[-3:])) == 1
            if settled:
                break
            time.sleep(0.25)
        if not settled:
            raise ImageError(f"APFS allocation did not settle for {self.path}: {readings}")
        stats = os.statvfs(self.path)
        if stats.f_frsize <= 0:
            raise ImageError(f"invalid statvfs fragment size for {self.path}")
        sample = SpaceSample(
            used_bytes=info.used_bytes,
            free_bytes=info.free_bytes,
            container_size_bytes=info.size_bytes,
            volume_capacity_in_use_bytes=info.volume_capacity_in_use_bytes,
            volume_used_bytes=(stats.f_blocks - stats.f_bfree) * stats.f_frsize,
            f_blocks=stats.f_blocks,
            f_bfree=stats.f_bfree,
            f_bavail=stats.f_bavail,
            f_frsize=stats.f_frsize,
            samples_used_bytes=tuple(readings),
            spread_bytes=max(readings) - min(readings),
            settled=settled,
            image_allocated_bytes=self.image_path.stat().st_blocks * 512,
        )
        return sample
