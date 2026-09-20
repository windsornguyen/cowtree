"""Measure experimental macOS directory clones of a quiescent prepared tree.

Apple discourages clonefile for directories. This probe is not a workspace backend:
it excludes Git registration, authority updates, and durability acknowledgement.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import shutil
import stat
import statistics
import sys
import time

from benchmarks.cargo_workspace import digest, inventory, verify
from cowtree.native import clonefile


@dataclass(frozen=True)
class Directory:
    mode: int
    mtime_ns: int


@dataclass(frozen=True)
class Sample:
    trial: int
    files: int
    directories: int
    logical_bytes: int
    clone_seconds: float
    materialize_seconds: float
    target_verify_seconds: float
    source_verify_seconds: float


@dataclass(frozen=True)
class Report:
    source_sha256: str
    trials: list[Sample]
    median_materialize_seconds: float


class Arguments(argparse.Namespace):
    source: Path
    root: Path
    trials: int


def directories(root: Path) -> dict[str, Directory]:
    """Record exact directory names and metadata, refusing hidden Git control state."""
    result: dict[str, Directory] = {}
    for directory, names, files in os.walk(root, followlinks=False):
        if ".git" in names or ".git" in files:
            raise ValueError("use a prepared tree without Git control files")
        path = Path(directory)
        metadata = path.stat()
        result[path.relative_to(root).as_posix()] = Directory(
            mode=stat.S_IMODE(metadata.st_mode), mtime_ns=metadata.st_mtime_ns
        )
    return result


def run(source: Path, root: Path, trials: int) -> list[Sample]:
    """Verify each complete clone and preserve failed trial directories for inspection."""
    if sys.platform != "darwin":
        raise ValueError("the primary clonefile directory operation requires macOS")
    if not 1 <= trials <= 10:
        raise ValueError("trials must be between 1 and 10")
    if source.is_symlink() or not source.is_dir():
        raise ValueError("source must be a quiescent directory, not a symlink")
    source = source.resolve()
    root = root.absolute()
    if os.path.lexists(root) or root.resolve().is_relative_to(source):
        raise ValueError("benchmark root must be absent and outside its source")
    expected_directories = directories(root=source)
    expected = inventory(root=source)
    root.mkdir()
    samples: list[Sample] = []
    for trial in range(trials):
        target = root / f"clone-{trial}"
        started = time.perf_counter()
        clonefile(source=source, target=target)
        clone_seconds = time.perf_counter() - started
        for name, metadata in reversed(expected_directories.items()):
            (target / name).chmod(metadata.mode)
            os.utime(target / name, ns=(metadata.mtime_ns, metadata.mtime_ns))
        materialize_seconds = time.perf_counter() - started
        checking = time.perf_counter()
        verify(root=target, expected=expected)
        if directories(root=target) != expected_directories:
            raise ValueError("clone directory names, modes, or modification times differ")
        target_verify_seconds = time.perf_counter() - checking
        checking = time.perf_counter()
        verify(root=source, expected=expected)
        if directories(root=source) != expected_directories:
            raise ValueError("source directory metadata changed")
        samples.append(
            Sample(
                trial=trial,
                files=len(expected),
                directories=len(expected_directories),
                logical_bytes=sum(entry.size for entry in expected.values()),
                clone_seconds=clone_seconds,
                materialize_seconds=materialize_seconds,
                target_verify_seconds=target_verify_seconds,
                source_verify_seconds=time.perf_counter() - checking,
            )
        )
        shutil.rmtree(target)
    report = Report(
        source_sha256=digest(expected),
        trials=samples,
        median_materialize_seconds=statistics.median(
            sample.materialize_seconds for sample in samples
        ),
    )
    (root / "report.json").write_text(json.dumps(asdict(report), indent=2) + "\n")
    return samples


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args(namespace=Arguments())
    samples = run(source=args.source, root=args.root, trials=args.trials)
    print(json.dumps([asdict(sample) for sample in samples], indent=2))


if __name__ == "__main__":
    main()
