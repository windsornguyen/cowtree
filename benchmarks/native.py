"""Compare Git, Python bindings, and the native CLI on the same complete operation.

Every method receives the same deterministic checkout. Trials rotate method
order, verify each target with Git outside the timer, and retire only owned
worktrees. Failed fixtures remain on disk for inspection.
"""

import argparse
from dataclasses import asdict, dataclass
from enum import Enum
import json
from pathlib import Path
import shutil
import statistics
import sys
import tempfile
import time

from run import BenchmarkRepository
from schema import BenchmarkProfile

from cowtree.core import add_worktree
from cowtree.exec import CommandRunner
from cowtree.types import WorktreeAddRequest


class Engine(str, Enum):
    GIT = "git"
    PYTHON_API = "python_api"
    PYTHON_CLI = "python_cli"
    RUST_CLI = "rust_cli"


@dataclass(frozen=True)
class Sample:
    engine: Engine
    trial: int
    seconds: float


@dataclass(frozen=True)
class Summary:
    engine: Engine
    median_seconds: float
    minimum_seconds: float
    maximum_seconds: float


@dataclass(frozen=True)
class Report:
    files: int
    worktrees: int
    samples: list[Sample]
    summary: list[Summary]


@dataclass(frozen=True)
class Workload:
    repository: BenchmarkRepository
    binary: Path
    worktrees: int

    def create(self, engine: Engine, target: Path) -> None:
        request = WorktreeAddRequest(path=target, source=self.repository.path)
        match engine:
            case Engine.PYTHON_API:
                add_worktree(request=request)
            case Engine.GIT:
                self.repository.io.run(
                    argv=[
                        "git",
                        "-C",
                        str(self.repository.path),
                        "worktree",
                        "add",
                        "--detach",
                        str(target),
                    ]
                )
            case Engine.PYTHON_CLI | Engine.RUST_CLI:
                command = (
                    [str(self.binary)]
                    if engine is Engine.RUST_CLI
                    else [
                        sys.executable,
                        "-c",
                        "from cowtree.cli import main; raise SystemExit(main())",
                    ]
                )
                self.repository.io.run(
                    argv=[*command, "add", "--detach", str(target)],
                    cwd=str(self.repository.path),
                )

    def measure(self, engine: Engine, trial: int) -> Sample:
        root = self.repository.path.parent / f"{engine.value}-{trial}"
        root.mkdir()
        targets = [root / f"tree-{index}" for index in range(self.worktrees)]
        started = time.perf_counter()
        for target in targets:
            self.create(engine=engine, target=target)
        seconds = time.perf_counter() - started
        for target in targets:
            status = self.repository.io.run(
                argv=["git", "-C", str(target), "status", "--porcelain"]
            )
            if status.stdout:
                raise ValueError(f"target differs from its commit: {target}")
            self.repository.io.run(
                argv=["git", "-C", str(self.repository.path), "worktree", "remove", str(target)]
            )
        root.rmdir()
        return Sample(engine=engine, trial=trial, seconds=seconds)


def run(binary: Path, files: int, worktrees: int, trials: int) -> Report:
    root = Path(tempfile.mkdtemp(prefix="cowtree-native-bench-"))
    profile = BenchmarkProfile(
        name="native",
        worktrees=worktrees,
        files=files,
        bytes_per_file=8192,
        description="matched native worktree fixture",
    )
    repository = BenchmarkRepository(path=root / "source", profile=profile, io=CommandRunner())
    repository.create()
    workload = Workload(repository=repository, binary=binary.resolve(), worktrees=worktrees)
    engines = list(Engine)
    samples: list[Sample] = []
    for trial in range(trials):
        offset = trial % len(engines)
        samples.extend(
            workload.measure(engine=engine, trial=trial)
            for engine in [*engines[offset:], *engines[:offset]]
        )
    summary: list[Summary] = []
    for engine in engines:
        seconds = [sample.seconds for sample in samples if sample.engine is engine]
        summary.append(Summary(engine, statistics.median(seconds), min(seconds), max(seconds)))
    status = repository.io.run(argv=["git", "-C", str(repository.path), "status", "--porcelain"])
    if status.stdout:
        raise ValueError(f"source changed: {root}")
    shutil.rmtree(root)
    return Report(files=files, worktrees=worktrees, samples=samples, summary=summary)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--files", type=int, default=512)
    parser.add_argument("--worktrees", type=int, default=4)
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.files <= 20000 or not 1 <= args.worktrees <= 64 or not 1 <= args.trials <= 9:
        parser.error("require 1..20000 files, 1..64 worktrees, and 1..9 trials")
    report = run(binary=args.binary, files=args.files, worktrees=args.worktrees, trials=args.trials)
    args.output.write_text(json.dumps(asdict(report), indent=2) + "\n")
    print(json.dumps([asdict(summary) for summary in report.summary], indent=2))


if __name__ == "__main__":
    main()
