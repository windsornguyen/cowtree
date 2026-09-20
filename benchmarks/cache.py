"""Compare fresh Git, shared mbx, and inherited Cargo targets on cowtree-metadata."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
from enum import Enum
import json
import os
from pathlib import Path
import signal
import subprocess
import time

from benchmarks.cargo_workspace import CargoMessage, clone_entries, inventory, verify
from cowtree.exec import CommandRunner
from cowtree.leaves import Leaves
from cowtree.tree_types import DerivedHardlinks, PathPolicy
from cowtree.version import VersionInfo
from cowtree.workspace import Workspace


class Lane(str, Enum):
    GIT = "git"
    COWTREE = "cowtree"
    MBX = "mbx"


@dataclass(frozen=True)
class Sample:
    lane: Lane
    trial: int
    create_seconds: float
    build_seconds: float
    fresh: int
    scheduled: int


@dataclass(frozen=True)
class Report:
    prime: Sample
    mbx_prime: Sample
    import_seconds: float
    samples: list[Sample]


class Arguments(argparse.Namespace):
    source: Path
    root: Path
    cargo: Path
    toolchain: str
    cargo_home: Path
    binary: Path
    mbx: Path
    trials: int


def build(arguments: Arguments, path: Path, lane: Lane, trial: int) -> Sample:
    """Retain compiler messages and check the actual executable outside the timer."""
    root = arguments.root
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("MBX_", "CARGO_", "RUSTFLAGS", "RUSTC_"))
    }
    environment.update(
        CARGO_HOME=str(arguments.cargo_home),
        RUSTC=str(arguments.cargo.parent / "rustc"),
        CARGO_TARGET_DIR=str(path / "target"),
        CARGO_BUILD_BUILD_DIR=str(path / "target"),
        CARGO_BUILD_JOBS="2",
        CARGO_INCREMENTAL="0",
        RUSTC_WRAPPER="",
        RUSTC_WORKSPACE_WRAPPER="",
    )
    if lane is Lane.MBX:
        environment.update(
            MBX_CACHE_DIR=str(root / "mbx-store"),
            MBX_TARGET_ROOT=str(root / "mbx-targets"),
            MBX_TARGET_VIEWS="0",
            MBX_GC_AUTO="0",
            MBX_INCREMENTAL="0",
            MBX_LINKER="system",
            MBX_STATS_REPORT=str(root / f"mbx-{trial}.json"),
        )
        command = [str(arguments.mbx), f"+{arguments.toolchain}"]
    else:
        command = [str(arguments.cargo)]
    command.extend(
        ["build", "--locked", "--offline", "-p", "cowtree-metadata", "--message-format=json"]
    )
    log = root / f"{lane.value}-{trial}.jsonl"
    started = time.perf_counter()
    with log.open("w") as output, log.with_suffix(".log").open("w") as errors:
        process = subprocess.Popen(  # noqa: S603 -- explicit executables in a trusted fixture.
            command, cwd=path, env=environment, stdout=output, stderr=errors, start_new_session=True
        )
        try:
            code = process.wait(timeout=180)
        except BaseException:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            raise
    elapsed = time.perf_counter() - started
    if code:
        raise RuntimeError(f"build exited {code}: {log.with_suffix('.log')}")
    messages = [CargoMessage.model_validate_json(line) for line in log.read_text().splitlines()]
    artifacts = [message for message in messages if message.reason == "compiler-artifact"]
    if not artifacts or any(message.fresh is None for message in artifacts):
        raise RuntimeError("Cargo omitted artifact freshness")
    binary = path / "target/debug/cowtree-metadata"
    checked = subprocess.run(  # noqa: S603 -- the fixture's compiled executable.
        [str(binary), "--version", "--json"], capture_output=True, text=True, check=True, timeout=10
    )
    if VersionInfo.model_validate_json(checked.stdout).version != "0.1.0":
        raise RuntimeError(f"unexpected fixture version: {checked.stdout}")
    result = Sample(
        lane,
        trial,
        0,
        elapsed,
        sum(m.fresh is True for m in artifacts),
        sum(m.fresh is False for m in artifacts),
    )
    return result


def worktree(source: Path, path: Path) -> None:
    """Create only a new detached worktree of the frozen seed."""
    CommandRunner().run(
        ["git", "-C", str(source), "worktree", "add", "--detach", str(path), "HEAD"]
    )


def run(arguments: Arguments) -> None:
    """Prime owned caches once, rotate trials, and preserve failure evidence."""
    root = arguments.root
    if os.path.lexists(root) or root.resolve().is_relative_to(arguments.source):
        raise ValueError("root must be absent and outside source")
    root.mkdir()
    seed = root / "seed"
    CommandRunner().run(["git", "clone", "--no-hardlinks", str(arguments.source), str(seed)])
    prime = build(arguments, seed, Lane.GIT, -1)
    before = inventory(root=seed)
    expected = clone_entries(source=seed, before=before, derived="target")
    started = time.perf_counter()
    workspace = Workspace.create(
        root=root / "store",
        source=seed,
        binary=arguments.binary,
        policy=PathPolicy(derived=("target",), derived_hardlinks=DerivedHardlinks.CLONE),
    )
    import_seconds = time.perf_counter() - started
    warm = root / "mbx-seed"
    worktree(seed, warm)
    mbx_prime = build(arguments, warm, Lane.MBX, -1)
    samples: list[Sample] = []
    for trial in range(arguments.trials):
        order = list(Lane) if trial % 2 == 0 else list(reversed(Lane))
        for lane in order:
            path = root / f"{lane.value}-{trial}"
            started = time.perf_counter()
            if lane is Lane.COWTREE:
                Leaves(workspace).fork(path=path)
            else:
                worktree(seed, path)
            creation = time.perf_counter() - started
            if lane is Lane.COWTREE:
                verify(root=path, expected=expected, cloned=True)
            measured = build(arguments, path, lane, trial)
            sample = Sample(
                lane, trial, creation, measured.build_seconds, measured.fresh, measured.scheduled
            )
            samples.append(sample)
            verify(root=seed, expected=before)
            report = Report(prime, mbx_prime, import_seconds, samples)
            (root / "report.json").write_text(json.dumps(asdict(report), indent=2) + "\n")
            print(json.dumps(asdict(sample)), flush=True)


def main() -> None:
    """Require explicit fixture, toolchain and cache paths for a portable run."""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "root", "cargo-home", "binary", "mbx"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--toolchain", default="1.97.1")
    parser.add_argument("--trials", type=int, choices=range(1, 11), default=3)
    arguments = parser.parse_args(namespace=Arguments())
    arguments.root = arguments.root.absolute()
    arguments.source = arguments.source.resolve()
    arguments.cargo_home = arguments.cargo_home.resolve()
    arguments.binary = arguments.binary.resolve()
    arguments.mbx = arguments.mbx.resolve()
    compiler = CommandRunner().run(["rustup", "which", "--toolchain", arguments.toolchain, "cargo"])
    arguments.cargo = Path(compiler.stdout.strip()).resolve()
    run(arguments)


if __name__ == "__main__":
    main()
