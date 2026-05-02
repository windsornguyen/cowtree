from __future__ import annotations

from dataclasses import asdict, dataclass
from enum import Enum
import json
from typing import cast


class Method(str, Enum):
    GIT = "git"
    COWTREE = "cowtree"


@dataclass(frozen=True)
class BenchmarkProfile:
    name: str
    worktrees: int
    files: int
    bytes_per_file: int
    description: str

    @property
    def tracked_bytes(self) -> int:
        value = self.files * self.bytes_per_file
        return value

    @property
    def fleet_payload_bytes(self) -> int:
        value = self.tracked_bytes * self.worktrees
        return value


@dataclass(frozen=True)
class PlatformInfo:
    system: str
    release: str
    machine: str
    python: str


@dataclass(frozen=True)
class BenchmarkRun:
    profile: str
    method: Method
    iteration: int
    elapsed_seconds: float
    worktrees: int
    files: int
    tracked_bytes: int
    fleet_payload_bytes: int
    payload_copied_bytes: int


@dataclass(frozen=True)
class BenchmarkSummary:
    profile: str
    method: Method
    runs: int
    median_elapsed_seconds: float
    min_elapsed_seconds: float
    max_elapsed_seconds: float
    worktrees: int
    files: int
    tracked_bytes: int
    fleet_payload_bytes: int
    payload_copied_bytes: int


@dataclass(frozen=True)
class BenchmarkResults:
    schema_version: int
    created_at: str
    platform: PlatformInfo
    profiles: list[BenchmarkProfile]
    runs: list[BenchmarkRun]
    summary: list[BenchmarkSummary]

    def to_json_text(self) -> str:
        raw = asdict(self)
        text = json.dumps(raw, indent=2) + "\n"
        return text

    @classmethod
    def from_json_text(cls, text: str) -> BenchmarkResults:
        raw = expect_mapping(json.loads(text))
        platform = platform_from_mapping(expect_mapping(raw["platform"]))
        profiles = [profile_from_mapping(expect_mapping(item)) for item in expect_list(raw["profiles"])]
        runs = [run_from_mapping(expect_mapping(item)) for item in expect_list(raw["runs"])]
        summary = [summary_from_mapping(expect_mapping(item)) for item in expect_list(raw["summary"])]
        results = cls(
            schema_version=expect_int(raw["schema_version"]),
            created_at=expect_str(raw["created_at"]),
            platform=platform,
            profiles=profiles,
            runs=runs,
            summary=summary,
        )
        return results


def platform_from_mapping(raw: dict[str, object]) -> PlatformInfo:
    platform = PlatformInfo(
        system=expect_str(raw["system"]),
        release=expect_str(raw["release"]),
        machine=expect_str(raw["machine"]),
        python=expect_str(raw["python"]),
    )
    return platform


def profile_from_mapping(raw: dict[str, object]) -> BenchmarkProfile:
    profile = BenchmarkProfile(
        name=expect_str(raw["name"]),
        worktrees=expect_int(raw["worktrees"]),
        files=expect_int(raw["files"]),
        bytes_per_file=expect_int(raw["bytes_per_file"]),
        description=expect_str(raw["description"]),
    )
    return profile


def run_from_mapping(raw: dict[str, object]) -> BenchmarkRun:
    run = BenchmarkRun(
        profile=expect_str(raw["profile"]),
        method=Method(expect_str(raw["method"])),
        iteration=expect_int(raw["iteration"]),
        elapsed_seconds=expect_float(raw["elapsed_seconds"]),
        worktrees=expect_int(raw["worktrees"]),
        files=expect_int(raw["files"]),
        tracked_bytes=expect_int(raw["tracked_bytes"]),
        fleet_payload_bytes=expect_int(raw["fleet_payload_bytes"]),
        payload_copied_bytes=expect_int(raw["payload_copied_bytes"]),
    )
    return run


def summary_from_mapping(raw: dict[str, object]) -> BenchmarkSummary:
    summary = BenchmarkSummary(
        profile=expect_str(raw["profile"]),
        method=Method(expect_str(raw["method"])),
        runs=expect_int(raw["runs"]),
        median_elapsed_seconds=expect_float(raw["median_elapsed_seconds"]),
        min_elapsed_seconds=expect_float(raw["min_elapsed_seconds"]),
        max_elapsed_seconds=expect_float(raw["max_elapsed_seconds"]),
        worktrees=expect_int(raw["worktrees"]),
        files=expect_int(raw["files"]),
        tracked_bytes=expect_int(raw["tracked_bytes"]),
        fleet_payload_bytes=expect_int(raw["fleet_payload_bytes"]),
        payload_copied_bytes=expect_int(raw["payload_copied_bytes"]),
    )
    return summary


def expect_mapping(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise TypeError(f"expected mapping, got {type(value).__name__}")
    mapping = cast(dict[str, object], value)
    return mapping


def expect_list(value: object) -> list[object]:
    if not isinstance(value, list):
        raise TypeError(f"expected list, got {type(value).__name__}")
    items = cast(list[object], value)
    return items


def expect_str(value: object) -> str:
    if not isinstance(value, str):
        raise TypeError(f"expected str, got {type(value).__name__}")
    text = value
    return text


def expect_int(value: object) -> int:
    if not isinstance(value, int):
        raise TypeError(f"expected int, got {type(value).__name__}")
    number = value
    return number


def expect_float(value: object) -> float:
    if not isinstance(value, int | float):
        raise TypeError(f"expected float, got {type(value).__name__}")
    number = float(value)
    return number
