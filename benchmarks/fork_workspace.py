"""Measure fresh forks separately from lookup of an already prepared leaf.

Use a quiescent existing store and its initial or explicitly retained node. No
publication, collection, retention changes, or source writers may run concurrently.
Prepared lookup excludes creation and provides no atomic worker-claim semantics.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import os
from pathlib import Path
import stat
import statistics
import time
from typing import Literal

from pydantic import Field

from benchmarks.cargo_workspace import digest, inventory, verify
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.leaves import Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.metadata_types import Digest, Record
from cowtree.seals import RetainedNode
from cowtree.workspace import Workspace
from cowtree.workspace_types import Node


class Config(Record):
    store: Path
    node: Digest
    root: Path
    trials: int = Field(default=3, ge=1, le=20)


class Trial(Record):
    leaf: int
    fresh_seconds: float
    prepared_lookup_seconds: float


class Report(Record):
    node: Digest
    source_sha256: str
    source_entries: int
    source_directories: int
    verified_files: int
    trials: tuple[Trial, ...]
    fresh_median_seconds: float
    prepared_lookup_median_seconds: float
    prepared_lookup_excludes_creation: Literal[True] = True
    atomic_worker_claim: Literal[False] = False


@dataclass(frozen=True)
class Directory:
    mode: int
    mtime_ns: int


def directories(root: Path) -> dict[str, Directory]:
    """Capture nested directory metadata; Git creates the leaf's root control entry."""
    result: dict[str, Directory] = {}
    for parent, names, _ in os.walk(root, followlinks=False):
        names[:] = [name for name in names if name != ".git"]
        for name in names:
            path = Path(parent) / name
            metadata = path.lstat()
            if stat.S_ISDIR(metadata.st_mode):
                result[path.relative_to(root).as_posix()] = Directory(
                    mode=stat.S_IMODE(metadata.st_mode), mtime_ns=metadata.st_mtime_ns
                )
    return result


def verify_directories(root: Path, expected: dict[str, Directory]) -> None:
    if directories(root=root) != expected:
        raise CowtreeError(
            CowtreeErrorCode.COMMAND_FAILED, f"directory names, modes, or times differ: {root}"
        )


def benchmark_node(workspace: Workspace, identity: Digest) -> Node:
    """Require a node whose retention outlives every benchmark trial."""
    node = workspace.nodes.read(identity=identity)
    if node.id != workspace.config.initial:
        path = workspace.root / "retained" / f"{node.id}.json"
        if (
            not path.is_file()
            or RetainedNode.model_validate_json(path.read_bytes()).node != node.id
        ):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "retain benchmark node first")
    return node


def run(config: Config) -> Report:
    """Keep receipts and remove only successfully created benchmark leaves."""
    workspace = Workspace.open(root=config.store)
    leaves = Leaves(workspace)
    with workspace.session():
        node = benchmark_node(workspace=workspace, identity=config.node)
        root = leaves.target(path=config.root)
        if root.is_relative_to(workspace.config.source.resolve()):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "output must be outside source")
        before_leaves = leaves.records()
    root.mkdir()
    tree = workspace.root / "nodes" / node.id / "tree"
    expected = inventory(root=tree)
    expected_directories = directories(root=tree)
    source = inventory(root=workspace.config.source)
    source_directories = directories(root=workspace.config.source)
    siblings = {leaf.id: inventory(root=leaf.path) for leaf in before_leaves}
    sibling_directories = {leaf.id: directories(root=leaf.path) for leaf in before_leaves}
    samples: list[Trial] = []
    for index in range(config.trials):
        started = time.perf_counter()
        leaf = leaves.fork(path=root / f"trial-{index}", node=node.id)
        fresh_seconds = time.perf_counter() - started
        try:
            started = time.perf_counter()
            prepared = Leaves(Workspace.open(root=config.store)).read(identity=leaf.id)
            identity = prepared.path.lstat()
            lookup_seconds = time.perf_counter() - started
            if prepared != leaf or (identity.st_dev, identity.st_ino) != (leaf.device, leaf.inode):
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "prepared leaf identity changed"
                )
            verify(root=leaf.path, expected=expected, cloned=True)
            verify_directories(root=leaf.path, expected=expected_directories)
            samples.append(
                Trial(
                    leaf=leaf.id,
                    fresh_seconds=fresh_seconds,
                    prepared_lookup_seconds=lookup_seconds,
                )
            )
        finally:
            Lifecycle(workspace).drop(identity=leaf.id)
        if leaves.records() != before_leaves:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "benchmark changed other leaves")
    verify(root=tree, expected=expected)
    verify_directories(root=tree, expected=expected_directories)
    verify(root=workspace.config.source, expected=source)
    verify_directories(root=workspace.config.source, expected=source_directories)
    for leaf in before_leaves:
        verify(root=leaf.path, expected=siblings[leaf.id])
        verify_directories(root=leaf.path, expected=sibling_directories[leaf.id])
    report = Report(
        node=node.id,
        source_sha256=digest(entries=expected),
        source_entries=len(expected),
        source_directories=len(expected_directories),
        verified_files=len(expected) * (config.trials + 1)
        + len(source)
        + sum(len(entries) for entries in siblings.values()),
        trials=tuple(samples),
        fresh_median_seconds=statistics.median(sample.fresh_seconds for sample in samples),
        prepared_lookup_median_seconds=statistics.median(
            sample.prepared_lookup_seconds for sample in samples
        ),
    )
    (root / "report.json").write_text(report.model_dump_json(indent=2) + "\n")
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--store", required=True, type=Path)
    parser.add_argument("--node", required=True)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--trials", type=int, default=3)
    args = parser.parse_args()
    report = run(
        config=Config(store=args.store, node=args.node, root=args.root, trials=args.trials)
    )
    print(report.model_dump_json(indent=2))


if __name__ == "__main__":
    main()
