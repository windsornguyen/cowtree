"""Measure frozen-tree import with an independent source and cache byte oracle."""

from __future__ import annotations

import argparse
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field
import hashlib
import json
import os
from pathlib import Path
import statistics
import time
from unittest.mock import patch

from pydantic import JsonValue

from cowtree.exec import CommandRunner
from cowtree.metadata import Metadata
from cowtree.metadata_types import Operation, Output
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace


@dataclass
class Measurement:
    elapsed_seconds: float = 0
    source_files: int = 0
    verified_files: int = 0
    operations: dict[str, int] = field(default_factory=dict)
    operation_seconds: dict[str, float] = field(default_factory=dict)


@contextmanager
def instrument(result: Measurement) -> Iterator[None]:
    original = Metadata.call

    def call(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        start = time.perf_counter()
        output = original(self, operation, payload)
        name = operation.value
        result.operations[name] = result.operations.get(name, 0) + 1
        result.operation_seconds[name] = (
            result.operation_seconds.get(name, 0) + time.perf_counter() - start
        )
        return output

    with patch.object(Metadata, "call", call):
        yield


def fixture(root: Path, count: int) -> Path:
    source = root / "source"
    source.mkdir()
    for arguments in (
        ["init", "-q"],
        ["config", "user.name", "Cowtree benchmark"],
        ["config", "user.email", "benchmark@example.com"],
        ["config", "commit.gpgsign", "false"],
    ):
        CommandRunner().run(argv=["git", "-C", str(source), *arguments])
    for index in range(count):
        (source / f"source-{index:05}.txt").write_bytes(
            hashlib.sha256(str(index).encode()).digest() * 32
        )
    CommandRunner().run(argv=["git", "-C", str(source), "add", "."])
    CommandRunner().run(argv=["git", "-C", str(source), "commit", "-qm", "fixture"])
    return source


def trial(root: Path, source: Path, binary: Path) -> Measurement:
    expected = {
        path.relative_to(source).as_posix(): os.fsencode(os.readlink(path))
        if path.is_symlink()
        else path.read_bytes()
        for path in source.rglob("*")
        if (path.is_file() or path.is_symlink()) and ".git" not in path.relative_to(source).parts
    }
    result = Measurement(source_files=len(expected))
    start = time.perf_counter()
    with instrument(result=result):
        workspace = Workspace.create(root=root, source=source, binary=binary, policy=PathPolicy())
    result.elapsed_seconds = time.perf_counter() - start
    node = workspace.nodes.read(identity=workspace.config.initial)
    for name, data in expected.items():
        target = root / "nodes" / node.id / "tree" / name
        assert (
            os.fsencode(os.readlink(target)) if target.is_symlink() else target.read_bytes()
        ) == data
        assert (root / "authority/objects" / node.source[name].object).read_bytes() == data
        original = source / name
        assert (
            os.fsencode(os.readlink(original)) if original.is_symlink() else original.read_bytes()
        ) == data
        result.verified_files += 3
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--files", type=int, default=128)
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument(
        "--source", type=Path, help="clean owned source checkout without ignored files"
    )
    args = parser.parse_args()
    args.root.mkdir()
    source = (
        args.source.resolve()
        if args.source is not None
        else fixture(root=args.root, count=args.files)
    )
    rows = [
        trial(root=args.root / f"trial-{index}", source=source, binary=args.binary.resolve())
        for index in range(args.trials)
    ]
    report = {
        "trials": [asdict(row) for row in rows],
        "median_seconds": statistics.median(row.elapsed_seconds for row in rows),
        "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        "source": str(source),
    }
    (args.root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
