"""Measure four warm target copies against Cowtree forks in isolated APFS images."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import shutil
import time

from benchmarks.apfs_space import ManagedImage
from benchmarks.cache import Lane, worktree
from benchmarks.cargo_workspace import clone_entries, inventory, verify
from cowtree.exec import CommandRunner
from cowtree.leaves import Leaves
from cowtree.tree_types import DerivedHardlinks, PathPolicy
from cowtree.workspace import Workspace


@dataclass(frozen=True)
class Sample:
    trial: int
    lane: Lane
    files_per_leaf: int
    logical_bytes_per_leaf: int
    additional_physical_bytes: int
    create_seconds: list[float]


class Arguments(argparse.Namespace):
    source: Path
    root: Path
    binary: Path
    trials: int


def measure(arguments: Arguments, trial: int, lane: Lane) -> Sample:
    """Include filesystem metadata and count shared extents once, outside creation timing."""
    io = CommandRunner()
    with ManagedImage(arguments.root, f"{lane.value}-{trial}", 2, io) as image:
        seed = image.path / "seed"
        io.run(["git", "clone", "--no-hardlinks", str(arguments.source), str(seed)])
        shutil.copytree(arguments.source / "target", seed / "target")
        before = inventory(root=seed)
        expected = clone_entries(source=seed, before=before, derived="target")
        workspace = None
        if lane is Lane.COWTREE:
            workspace = Workspace.create(
                root=image.path / "store",
                source=seed,
                binary=arguments.binary,
                policy=PathPolicy(derived=("target",), derived_hardlinks=DerivedHardlinks.CLONE),
            )
        baseline = image.sample()
        seconds: list[float] = []
        for number in range(4):
            path = image.path / f"leaf-{number}"
            started = time.perf_counter()
            if workspace is None:
                worktree(seed, path)
                shutil.copytree(seed / "target", path / "target")
                for name, entry in expected.items():
                    if not name.startswith("target/") and entry.kind == "file":
                        shutil.copy2(seed / name, path / name)
            else:
                Leaves(workspace).fork(path=path)
            seconds.append(time.perf_counter() - started)
            verify(root=path, expected=expected, cloned=True)
        final = image.sample()
        verify(root=seed, expected=before)
        measurements = {"baseline": asdict(baseline), "final": asdict(final)}
        (arguments.root / f"{lane.value}-{trial}.json").write_text(
            json.dumps(measurements, indent=2) + "\n"
        )
        result = Sample(
            trial,
            lane,
            len(expected),
            sum(entry.size for entry in expected.values()),
            final.used_bytes - baseline.used_bytes,
            seconds,
        )
    return result


def main() -> None:
    """Require a quiescent seed from the cache comparison and retain detached images."""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "root", "binary"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--trials", type=int, choices=range(1, 11), default=3)
    arguments = parser.parse_args(namespace=Arguments())
    arguments.root = arguments.root.absolute()
    arguments.source = arguments.source.resolve()
    arguments.binary = arguments.binary.resolve()
    if os.path.lexists(arguments.root) or arguments.root.resolve().is_relative_to(arguments.source):
        raise ValueError("root must be absent and outside source")
    if (arguments.source / "target").is_symlink() or not (arguments.source / "target").is_dir():
        raise ValueError("source must contain a real private target directory")
    arguments.root.mkdir()
    samples: list[Sample] = []
    for trial in range(arguments.trials):
        for lane in (Lane.GIT, Lane.COWTREE):
            sample = measure(arguments, trial, lane)
            samples.append(sample)
            (arguments.root / "report.json").write_text(
                json.dumps([asdict(row) for row in samples], indent=2) + "\n"
            )
            print(json.dumps(asdict(sample)), flush=True)


if __name__ == "__main__":
    main()
