"""Qualify seeded managed workspace histories against an independent byte model."""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch

from pydantic import JsonValue, TypeAdapter

from cowtree.batches import Batches
from cowtree.captures import Captures
from cowtree.collection import Collector
from cowtree.leaves import Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.metadata import Metadata
from cowtree.metadata_types import BatchCandidate, Entry, Operation, Output, Tip
from cowtree.seals import Seals
from cowtree.tree_types import PathPolicy
from cowtree.views import Views
from cowtree.workspace import Workspace


CACHE = "history-cache/payload"


@dataclass(frozen=True)
class Config:
    seed: int
    rounds: int
    files: int
    cache_bytes: int
    binary: Path
    source: Path | None = None


@dataclass(frozen=True)
class Value:
    kind: str
    data: bytes

    def digest(self) -> str:
        return hashlib.sha256(self.data).hexdigest()


@dataclass
class View:
    path: Path
    entries: dict[str, Value]
    origins: dict[str, Value]


def scan(root: Path) -> dict[str, Value]:
    """Read real bytes and Unix modes without calling cowtree's manifest machinery."""
    entries: dict[str, Value] = {}
    for directory, names, files in os.walk(root, followlinks=False):
        names[:] = [name for name in names if name != ".git"]
        for name in [*names, *files]:
            if name == ".git":
                continue
            path = Path(directory) / name
            metadata = path.lstat()
            if stat.S_ISLNK(metadata.st_mode):
                value = Value(kind="symlink", data=os.fsencode(os.readlink(path)))
            elif stat.S_ISREG(metadata.st_mode):
                kind = "executable" if metadata.st_mode & stat.S_IXUSR else "file"
                value = Value(kind=kind, data=path.read_bytes())
            elif stat.S_ISDIR(metadata.st_mode):
                continue
            else:
                raise AssertionError(f"unsupported entry: {path}")
            entries[path.relative_to(root).as_posix()] = value
    return entries


def manifest(entries: dict[str, Value]) -> dict[str, dict[str, str]]:
    return {
        path: {"kind": value.kind, "object": value.digest()}
        for path, value in sorted(entries.items())
    }


def identity(entries: dict[str, Value]) -> str:
    return hashlib.sha256(json.dumps(manifest(entries), sort_keys=True).encode()).hexdigest()


def git(root: Path, *arguments: str) -> str:
    binary = shutil.which("git")
    if binary is None:
        raise RuntimeError("Git executable is required")
    result = subprocess.run(  # noqa: S603 - explicit argv for owned benchmark repositories.
        [binary, "-C", str(root), *arguments], capture_output=True, check=True, text=True
    )
    return result.stdout.strip()


def fixture(root: Path, config: Config) -> dict[str, Value]:
    root.mkdir()
    if config.source is None:
        git(root, "init", "-q")
    else:
        git(
            root.parent,
            "clone",
            "--local",
            "--no-hardlinks",
            "--quiet",
            "--",
            str(config.source.resolve()),
            str(root),
        )
    git(root, "config", "user.email", "benchmark@example.invalid")
    git(root, "config", "user.name", "Cowtree benchmark")
    git(root, "config", "commit.gpgsign", "false")
    source = root / "history-source"
    source.mkdir()
    for index in range(config.files):
        (source / f"{index:04}.txt").write_bytes(
            (f"seed={config.seed} file={index}\n" * 128).encode()
        )
    (source / "anchor.txt").write_bytes(b"stable symlink target\n")
    ignored = root / ".gitignore"
    with ignored.open("a") as stream:
        stream.write("\n/history-cache/\n")
    git(root, "add", ".")
    git(root, "commit", "-qm", "workspace history fixture")
    (root / "history-cache").mkdir()
    generator = random.Random(config.seed)  # noqa: S311 - reproducible workload, not credentials.
    (root / CACHE).write_bytes(generator.randbytes(config.cache_bytes))
    return scan(root=root)


def runtime_identity() -> str:
    """Hash loaded cowtree Python modules and metadata source, including dirty edits."""
    repository = Path(__file__).resolve().parents[1]
    paths = {
        Path(filename).resolve()
        for name, module in tuple(sys.modules.items())
        if name.startswith("cowtree.") and (filename := getattr(module, "__file__", None))
    }
    assert all(path.is_relative_to(repository / "src") for path in paths), "run with PYTHONPATH=src"
    paths.update((repository / "crates/metadata/src").glob("*"))
    paths.add(repository / "Cargo.lock")
    paths.add(Path(__file__).resolve())
    digest = hashlib.sha256()
    for path in sorted(path for path in paths if path.is_file()):
        digest.update(str(path.relative_to(repository)).encode() + b"\0")
        digest.update(path.read_bytes())
    return digest.hexdigest()


@dataclass
class History:
    workspace: Workspace
    random: random.Random
    published: dict[str, Value]
    warm: Value
    live: dict[int, View] = field(default_factory=dict)
    retained: dict[str, View] = field(default_factory=dict)
    events: list[dict[str, JsonValue]] = field(default_factory=list)
    counts: dict[str, int] = field(default_factory=dict)
    version: int = 1

    def check(self, action: str) -> None:
        started = time.monotonic()
        for view in [*self.live.values(), *self.retained.values()]:
            actual = scan(root=view.path)
            assert actual == view.entries, f"{action}: byte/mode mismatch in {view.path}"
            self.counts["file_checks"] = self.counts.get("file_checks", 0) + len(actual)
        current = Workspace.open(root=self.workspace.root)
        warm = scan(root=current.root / "nodes" / current.config.warm_tip / "tree")
        assert warm == {**self.published, CACHE: self.warm}, f"{action}: warm tip changed"
        self.counts["file_checks"] = self.counts.get("file_checks", 0) + len(warm)
        with current.session() as metadata:
            tip = metadata.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
            assert tip.version == self.version, f"{action}: unexpected epoch"
            entries = metadata.call(Operation.SNAPSHOT, {"version": tip.version}).decode(
                "snapshot", TypeAdapter(dict[str, Entry])
            )
            assert {
                path: entry.model_dump(mode="json") for path, entry in entries.items()
            } == manifest(self.published)
            live = metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int]))
            assert set(live) == set(self.live), f"{action}: leaf inventory differs"
        self.counts[action] = self.counts.get(action, 0) + 1
        self.events.append(
            {
                "action": action,
                "version": self.version,
                "live": len(self.live),
                "retained": len(self.retained),
                "check_seconds": time.monotonic() - started,
            }
        )

    def fork(self, name: str, node: str | None = None) -> int:
        leaf = Leaves(self.workspace).fork(path=self.workspace.root.parent / name, node=node)
        if node is None:
            entries, origins = {**self.published, CACHE: self.warm}, dict(self.published)
        else:
            snapshot = self.retained[node]
            entries, origins = dict(snapshot.entries), dict(snapshot.origins)
        self.live[leaf.id] = View(path=leaf.path, entries=entries, origins=origins)
        self.check(action="fork_checkpoint" if node is not None else "fork_warm")
        return leaf.id

    def edit(self, leaf: int, path: str, value: Value | None) -> None:
        view = self.live[leaf]
        target = view.path / path
        if os.path.lexists(target):
            target.unlink()
        if value is None:
            view.entries.pop(path, None)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            if value.kind == "symlink":
                target.symlink_to(os.fsdecode(value.data))
            else:
                target.write_bytes(value.data)
                target.chmod(0o755 if value.kind == "executable" else 0o644)
            view.entries[path] = value
        self.check(action="delete" if value is None else f"edit_{value.kind}")

    def checkpoint(self, leaf: int) -> str:
        node = Seals(self.workspace).seal(identity=leaf)
        view = self.live[leaf]
        self.retained[node.id] = View(
            path=self.workspace.root / "nodes" / node.id / "tree",
            entries=dict(view.entries),
            origins=dict(view.origins),
        )
        self.check(action="checkpoint")
        return node.id

    def sync(self, leaf: int) -> None:
        view = self.live[leaf]
        source = {path: value for path, value in view.entries.items() if path != CACHE}
        dirty = {
            path
            for path in set(source) | set(view.origins)
            if source.get(path) != view.origins.get(path)
        }
        Views(self.workspace).sync(identity=leaf)
        for path in (set(source) | set(view.origins) | set(self.published)) - dirty:
            if path in self.published:
                view.entries[path] = self.published[path]
                view.origins[path] = self.published[path]
            else:
                view.entries.pop(path, None)
                view.origins.pop(path, None)
        self.check(action="sync")

    def drop(self, leaf: int) -> None:
        view = self.live.pop(leaf)
        Lifecycle(self.workspace).drop(identity=leaf, force=True)
        assert not view.path.exists()
        self.check(action="drop")

    def collect(self) -> None:
        result = Collector(self.workspace).collect()
        self.counts["nodes_collected"] = self.counts.get("nodes_collected", 0) + len(result.nodes)
        self.check(action="collect")

    def reclaim(self, keep: int) -> None:
        victims = sorted(set(self.live) - {keep})
        self.random.shuffle(victims)
        for victim in victims:
            self.drop(leaf=victim)
        if len(self.retained) > 2:
            released = self.random.choice(list(self.retained))
            Seals(self.workspace).release(identity=released)
            del self.retained[released]
            self.check(action="release_checkpoint")
        self.collect()
        self.workspace = Workspace.open(root=self.workspace.root)
        self.check(action="reopen")


VALIDATOR = """
import hashlib, json, os, stat, sys
from pathlib import Path
expected = json.loads(Path(sys.argv[1]).read_text())
actual = {}
for directory, names, files in os.walk('.', followlinks=False):
    names[:] = [name for name in names if name != '.git']
    for name in [*names, *files]:
        path = Path(directory) / name
        if name == '.git' or (path.is_dir() and not path.is_symlink()):
            continue
        executable = not path.is_symlink() and path.stat().st_mode & stat.S_IXUSR
        kind = 'symlink' if path.is_symlink() else ('executable' if executable else 'file')
        data = os.fsencode(os.readlink(path)) if path.is_symlink() else path.read_bytes()
        actual[path.as_posix()] = {'kind': kind, 'object': hashlib.sha256(data).hexdigest()}
assert actual == expected, 'validation union differs from independent model'
Path('history-cache/payload').write_bytes(Path(sys.argv[2]).read_bytes())
"""


def interrupt_commit(history: History, candidate: BatchCandidate) -> None:
    original = Metadata.call

    def lost_reply(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        response = original(self, operation, payload)
        if operation is Operation.COMMIT_BATCH:
            raise InterruptedError("injected lost batch acknowledgement")
        return response

    with patch.object(Metadata, "call", lost_reply):
        try:
            Batches(history.workspace).commit(candidate=candidate)
        except InterruptedError:
            history.counts["lost_replies"] = history.counts.get("lost_replies", 0) + 1
    history.workspace = Workspace.open(root=history.workspace.root)


def publish_round(history: History, number: int, cache_bytes: int) -> None:
    writers = [history.fork(name=f"writer-{number}-{index}") for index in range(2)]
    paths = sorted(
        path
        for path in history.published
        if path.startswith("history-source/") and path != "history-source/anchor.txt"
    )
    selected = history.random.sample(paths, 2)
    changes: dict[str, Value | None] = {}
    for index, (writer, path) in enumerate(zip(writers, selected, strict=True)):
        kind = (number * 2 + index) % 4
        value = (
            None
            if kind == 1
            else Value(
                kind="symlink" if kind == 3 else "executable" if kind == 2 else "file",
                data=b"anchor.txt" if kind == 3 else history.random.randbytes(257),
            )
        )
        history.edit(leaf=writer, path=path, value=value)
        changes[path] = value
        Captures(history.workspace).capture(identity=writer)
        history.check(action="capture")
    expected = dict(history.published)
    for path, value in changes.items():
        if value is None:
            expected.pop(path, None)
        else:
            expected[path] = value
    batches = Batches(history.workspace)
    candidate = batches.prepare(identities=tuple(writers))
    history.check(action="prepare_batch")
    root = history.workspace.root.parent
    expected_path = root / "expected.json"
    expected_path.write_text(json.dumps(manifest({**expected, CACHE: history.warm})))
    payload = history.random.randbytes(cache_bytes)
    (root / "next-cache.bin").write_bytes(payload)
    batches.validate(
        candidate=candidate,
        command=(sys.executable, "-c", VALIDATOR, str(expected_path), str(root / "next-cache.bin")),
    )
    history.check(action="validate_batch")
    for writer, path in zip(writers, selected, strict=True):
        history.edit(
            leaf=writer, path=path, value=Value(kind="file", data=history.random.randbytes(129))
        )
    if number % 3 == 0:
        interrupt_commit(history=history, candidate=candidate)
    result = batches.commit(candidate=candidate)
    history.published = expected
    history.warm = Value(kind="file", data=payload)
    history.version += 1
    assert {receipt.version for receipt in result.receipts} == {history.version}
    for writer, path in zip(writers, selected, strict=True):
        if changes[path] is None:
            history.live[writer].origins.pop(path, None)
        else:
            history.live[writer].origins[path] = expected[path]
    history.check(action="commit_batch")
    for writer in writers:
        history.sync(leaf=writer)
    assert batches.commit(candidate=candidate) == result
    history.check(action="retry_batch")


def run(root: Path, config: Config) -> dict[str, JsonValue]:
    if config.rounds < 1 or config.files < config.rounds + 2 or config.cache_bytes < 1:
        raise ValueError("positive rounds/cache and at least rounds + 2 source files required")
    started = time.monotonic()
    root.mkdir()
    initial = fixture(root=root / "source", config=config)
    workspace = Workspace.create(
        root=root / "workspace",
        source=root / "source",
        binary=config.binary,
        policy=PathPolicy(derived=("history-cache",)),
    )
    history = History(
        workspace=workspace,
        random=random.Random(config.seed),  # noqa: S311 - seeded history selection.
        published={path: value for path, value in initial.items() if path != CACHE},
        warm=initial[CACHE],
    )
    runtime_before = runtime_identity()
    binary_before = hashlib.sha256(config.binary.read_bytes()).hexdigest()
    history.check(action="initialize")
    for number in range(config.rounds):
        private = history.fork(name=f"private-{number}")
        history.edit(
            leaf=private,
            path=f"private-{number}.txt",
            value=Value(kind="file", data=history.random.randbytes(99)),
        )
        history.edit(
            leaf=private,
            path=CACHE,
            value=Value(kind="file", data=history.random.randbytes(config.cache_bytes)),
        )
        node = history.checkpoint(leaf=private)
        child = history.fork(name=f"checkpoint-{number}", node=node)
        publish_round(history=history, number=number, cache_bytes=config.cache_bytes)
        history.sync(leaf=child)
        reader = history.fork(name=f"reader-{number}")
        history.reclaim(keep=reader)
    runtime_after = runtime_identity()
    assert runtime_before == runtime_after, "runtime source changed during qualification"
    binary_after = hashlib.sha256(config.binary.read_bytes()).hexdigest()
    assert binary_before == binary_after, "metadata executable changed during qualification"
    return {
        "schema_version": 1,
        "python_version": sys.version,
        "seed": config.seed,
        "rounds": config.rounds,
        "fixture": "synthetic" if config.source is None else str(config.source.resolve()),
        "source_base_commit": None
        if config.source is None
        else git(root / "source", "rev-parse", "HEAD^"),
        "source_files": len(initial) - 1,
        "source_bytes": sum(len(value.data) for path, value in initial.items() if path != CACHE),
        "cache_bytes": config.cache_bytes,
        "fixture_source_sha256": identity(initial),
        "runtime_source_sha256": runtime_after,
        "runtime_git_head": git(Path(__file__).resolve().parents[1], "rev-parse", "HEAD"),
        "metadata_binary_sha256": binary_after,
        "published_sha256": identity(history.published),
        "counts": dict(history.counts),
        "events": list(history.events),
        "elapsed_seconds": time.monotonic() - started,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--files", type=int, default=32)
    parser.add_argument("--cache-mib", type=int, default=1)
    parser.add_argument("--source", type=Path)
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "target/debug/cowtree-metadata",
    )
    parser.add_argument("--output", type=Path, required=True)
    options = parser.parse_args()
    config = Config(
        seed=options.seed,
        rounds=options.rounds,
        files=options.files,
        cache_bytes=options.cache_mib * 1024 * 1024,
        binary=options.binary.resolve(),
        source=options.source,
    )
    with tempfile.TemporaryDirectory(prefix="cowtree-history-") as directory:
        receipt = run(root=Path(directory) / "trial", config=config)
    receipt["command"] = [sys.executable, *sys.argv]
    receipt["environment"] = {"PYTHONPATH": str(Path(__file__).resolve().parents[1] / "src")}
    options.output.parent.mkdir(parents=True, exist_ok=True)
    options.output.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({key: value for key, value in receipt.items() if key != "events"}, indent=2))


if __name__ == "__main__":
    main()
