"""Measure real make/cc cache reuse in warm forks against ordinary Git worktrees."""

from __future__ import annotations

import argparse
from pathlib import Path
import platform
import shutil
import time

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.leaves import Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.metadata_types import Record
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace


MAKEFILE = """CC = cc
CFLAGS = -O2 -Wall -Wextra -Werror

all: build/app

build/app: build/main.o build/value.o
	@echo COWTREE_LINK
	@$(CC) $^ -o $@

build/%.o: %.c | build
	@echo COWTREE_COMPILE:$<
	@$(CC) $(CFLAGS) -c $< -o $@

build:
	@mkdir build
"""
ARTIFACTS = ("build/main.o", "build/value.o", "build/app")


class Build(Record):
    seconds: float
    compiled: list[str]
    links: int
    output: int


class WorktreeBuild(Record):
    creation_seconds: float
    initial: Build
    changed: Build
    inherited_artifacts: bool
    unchanged_object_preserved: bool


class Trial(Record):
    seed_build: Build
    workspace_creation_seconds: float
    ordinary: WorktreeBuild
    cowtree: WorktreeBuild


class Benchmark(Record):
    platform: str
    compiler: str
    make: str
    trials: list[Trial]


def fixture(root: Path) -> None:
    root.mkdir()
    files = {
        "Makefile": MAKEFILE,
        ".gitignore": "build/\n",
        "main.c": (
            "#include <stdio.h>\nint value(void);\n"
            'int main(void) { printf("%d\\n", value()); return 0; }\n'
        ),
        "value.c": "int value(void) { return 42; }\n",
    }
    for name, text in files.items():
        (root / name).write_text(text)
    runner = CommandRunner()
    for arguments in (
        ["init", "-q"],
        ["config", "user.email", "benchmark@example.com"],
        ["config", "user.name", "Cowtree benchmark"],
        ["config", "commit.gpgsign", "false"],
        ["add", "."],
        ["commit", "-qm", "C build fixture"],
    ):
        runner.run(argv=["git", "-C", str(root), *arguments])


def build(root: Path) -> Build:
    runner = CommandRunner()
    started = time.perf_counter()
    command = runner.run(argv=["make", "--no-print-directory", "-j1"], cwd=str(root))
    elapsed = time.perf_counter() - started
    lines = command.stdout.splitlines()
    executable = runner.run(argv=[str(root / "build/app")], cwd=str(root))
    result = Build(
        seconds=elapsed,
        compiled=[
            line.removeprefix("COWTREE_COMPILE:")
            for line in lines
            if line.startswith("COWTREE_COMPILE:")
        ],
        links=lines.count("COWTREE_LINK"),
        output=int(executable.stdout.strip()),
    )
    return result


def measure(root: Path, creation_seconds: float, inherited: dict[str, int]) -> WorktreeBuild:
    before = {
        name: (root / name).stat().st_mtime_ns for name in ARTIFACTS if (root / name).exists()
    }
    initial = build(root=root)
    after = {name: (root / name).stat().st_mtime_ns for name in ARTIFACTS}
    warm = before == inherited
    expected = [] if warm else ["main.c", "value.c"]
    if initial.compiled != expected or initial.links != (0 if warm else 1) or initial.output != 42:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, "initial build violated its cache contract"
        )
    if warm and after != inherited:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, "warm build changed inherited artifacts"
        )
    # Accommodate make versions that compare whole-second timestamps; outside the timed build.
    time.sleep(max(0, max(after.values()) // 1_000_000_000 + 1 - time.time()))
    (root / "value.c").write_text("int value(void) { return 43; }\n")
    changed = build(root=root)
    preserved = (root / "build/main.o").stat().st_mtime_ns == after["build/main.o"]
    if (
        changed.compiled != ["value.c"]
        or changed.links != 1
        or changed.output != 43
        or not preserved
    ):
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, "source change did not rebuild exactly its dependents"
        )
    result = WorktreeBuild(
        creation_seconds=creation_seconds,
        initial=initial,
        changed=changed,
        inherited_artifacts=warm,
        unchanged_object_preserved=preserved,
    )
    return result


def trial(root: Path, binary: Path) -> Trial:
    root.mkdir()
    source = root / "source"
    fixture(root=source)
    seed = build(root=source)
    inherited = {name: (source / name).stat().st_mtime_ns for name in ARTIFACTS}
    started = time.perf_counter()
    workspace = Workspace.create(
        root=root / "workspace", source=source, binary=binary, policy=PathPolicy(derived=("build",))
    )
    setup_seconds = time.perf_counter() - started
    runner = CommandRunner()
    started = time.perf_counter()
    runner.run(
        argv=[
            "git",
            "-C",
            str(source),
            "worktree",
            "add",
            "--detach",
            "--",
            str(root / "ordinary"),
            "HEAD",
        ]
    )
    ordinary_seconds = time.perf_counter() - started
    started = time.perf_counter()
    leaves = Leaves(workspace)
    leaf = leaves.fork(path=root / "warm")
    warm_seconds = time.perf_counter() - started
    ordinary = measure(
        root=root / "ordinary", creation_seconds=ordinary_seconds, inherited=inherited
    )
    warm = measure(root=leaf.path, creation_seconds=warm_seconds, inherited=inherited)
    if ordinary.inherited_artifacts or not warm.inherited_artifacts:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, "worktree cache inheritance differs from the fixture"
        )
    if (
        build(root=source).compiled
        or {name: (source / name).stat().st_mtime_ns for name in ARTIFACTS} != inherited
    ):
        raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "private builds changed the seed cache")
    Lifecycle(workspace).drop(identity=leaf.id, force=True)
    runner.run(
        argv=[
            "git",
            "-C",
            str(source),
            "worktree",
            "remove",
            "--force",
            "--",
            str(root / "ordinary"),
        ]
    )
    result = Trial(
        seed_build=seed, workspace_creation_seconds=setup_seconds, ordinary=ordinary, cowtree=warm
    )
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", required=True, type=Path, help="absent scratch directory on native CoW filesystem"
    )
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    if arguments.trials <= 0:
        parser.error("--trials must be positive")
    for tool in ("cc", "make", "git"):
        if shutil.which(tool) is None:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, f"required tool unavailable: {tool}"
            )
    root = arguments.root.resolve()
    root.mkdir()
    runner = CommandRunner()
    result = Benchmark(
        platform=platform.platform(),
        compiler=runner.run(argv=["cc", "--version"]).stdout.splitlines()[0],
        make=runner.run(argv=["make", "--version"]).stdout.splitlines()[0],
        trials=[
            trial(root=root / str(index), binary=arguments.binary.resolve())
            for index in range(arguments.trials)
        ],
    )
    arguments.output.write_text(result.model_dump_json(indent=2) + "\n")
    print(result.model_dump_json(indent=2))


if __name__ == "__main__":
    main()
