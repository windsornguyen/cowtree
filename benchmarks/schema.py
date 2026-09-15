from __future__ import annotations

from dataclasses import asdict, dataclass
from enum import Enum
import json
from typing import TypeAlias, cast


JSONValue: TypeAlias = str | int | float | bool | None | list["JSONValue"] | dict[str, "JSONValue"]
JSONObject: TypeAlias = dict[str, JSONValue]


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

    @classmethod
    def from_mapping(cls, raw: JSONObject) -> BenchmarkProfile:
        profile = cls(
            name=expect_str(raw["name"]),
            worktrees=expect_int(raw["worktrees"]),
            files=expect_int(raw["files"]),
            bytes_per_file=expect_int(raw["bytes_per_file"]),
            description=expect_str(raw["description"]),
        )
        return profile


@dataclass(frozen=True)
class PlatformInfo:
    system: str
    release: str
    machine: str
    python: str

    @classmethod
    def from_mapping(cls, raw: JSONObject) -> PlatformInfo:
        platform = cls(
            system=expect_str(raw["system"]),
            release=expect_str(raw["release"]),
            machine=expect_str(raw["machine"]),
            python=expect_str(raw["python"]),
        )
        return platform


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

    @classmethod
    def from_mapping(cls, raw: JSONObject) -> BenchmarkRun:
        run = cls(
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

    @classmethod
    def from_mapping(cls, raw: JSONObject) -> BenchmarkSummary:
        summary = cls(
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
        platform = PlatformInfo.from_mapping(raw=expect_mapping(raw["platform"]))
        profiles = [
            BenchmarkProfile.from_mapping(raw=expect_mapping(item))
            for item in expect_list(raw["profiles"])
        ]
        runs = [
            BenchmarkRun.from_mapping(raw=expect_mapping(item)) for item in expect_list(raw["runs"])
        ]
        summary = [
            BenchmarkSummary.from_mapping(raw=expect_mapping(item))
            for item in expect_list(raw["summary"])
        ]
        results = cls(
            schema_version=expect_int(raw["schema_version"]),
            created_at=expect_str(raw["created_at"]),
            platform=platform,
            profiles=profiles,
            runs=runs,
            summary=summary,
        )
        return results


def expect_mapping(value: JSONValue) -> JSONObject:
    if not isinstance(value, dict):
        raise TypeError(f"expected mapping, got {type(value).__name__}")
    mapping = cast(JSONObject, value)
    return mapping


def expect_list(value: JSONValue) -> list[JSONValue]:
    if not isinstance(value, list):
        raise TypeError(f"expected list, got {type(value).__name__}")
    items = cast(list[JSONValue], value)
    return items


def expect_str(value: JSONValue) -> str:
    if not isinstance(value, str):
        raise TypeError(f"expected str, got {type(value).__name__}")
    text = value
    return text


def expect_int(value: JSONValue) -> int:
    if not isinstance(value, int):
        raise TypeError(f"expected int, got {type(value).__name__}")
    number = value
    return number


def expect_float(value: JSONValue) -> float:
    if not isinstance(value, int | float):
        raise TypeError(f"expected float, got {type(value).__name__}")
    number = float(value)
    return number
