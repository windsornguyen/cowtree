"""Run deterministic worktree mutations with a filesystem and Git state oracle.

Operations run to completion before the next operation starts. Directory loss is
an external deletion followed by explicit registry cleanup, not a crash model.
Verification hashes every tracked file in full mode; sampled mode hashes all
edited files and a fixed sample of clean files. Neither mode compiles the source.

The paired source fixture must use the same checkout permission policy as Git's
new worktrees. Git records executable bits, while checkout applies the umask;
the oracle deliberately compares full source permissions rather than ignoring
differences in group or other access bits.
"""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field
from enum import Enum
import hashlib
import json
import math
import os
from pathlib import Path
import random
import shutil
import stat
import subprocess
import time

from benchmarks.schema import Method
from cowtree.core import add_worktree, list_all_worktrees, remove_worktree
from cowtree.types import WorktreeAddRequest


class EditMode(str, Enum):
    ATOMIC_SAVE = "atomic-save"
    IN_PLACE_APPEND = "in-place-append"


class OracleError(RuntimeError):
    """The observed filesystem or Git state differs from the workload model."""


@dataclass(frozen=True)
class FleetConfig:
    repo: Path
    root: Path
    method: Method
    seed: int
    history: Path


@dataclass(frozen=True)
class MutationConfig:
    round_id: int
    fraction: float = 0.01
    mode: EditMode = EditMode.ATOMIC_SAVE

    def __post_init__(self) -> None:
        if not 0 < self.fraction <= 1:
            raise ValueError("fraction must be in (0, 1]")
        if self.round_id < 0:
            raise ValueError("round_id must be nonnegative")


@dataclass(frozen=True)
class FileState:
    mode: int
    size: int
    sha256: str
    symlink: bool

    @classmethod
    def read(cls, path: Path) -> FileState:
        """Hash bytes without following symlinks or retaining whole source files."""
        mode = path.lstat().st_mode
        digest = hashlib.sha256()
        if stat.S_ISLNK(mode):
            buf = os.fsencode(os.readlink(path))
            digest.update(buf)
            size = len(buf)
        elif stat.S_ISREG(mode):
            size = 0
            with path.open("rb") as src:
                for buf in iter(lambda: src.read(1024 * 1024), b""):
                    digest.update(buf)
                    size += len(buf)
        else:
            raise OracleError(f"unsupported tracked file type: {path}")
        result = cls(stat.S_IMODE(mode), size, digest.hexdigest(), stat.S_ISLNK(mode))
        return result


@dataclass
class Leaf:
    slot: int
    generation: int
    path: Path
    changed: dict[str, FileState] = field(default_factory=dict)


@dataclass(frozen=True)
class MutationStats:
    files_edited: int
    bytes_written: int
    source_bytes_selected: int
    selection_sha256: str


@dataclass(frozen=True)
class VerificationStats:
    full: bool
    active_worktrees: int
    files_hashed: int
    bytes_hashed: int
    logical_bytes: int
    changed_files: int
    state_sha256: str
    history_operations: int


@dataclass(frozen=True)
class HistoryEvent:
    sequence: int
    phase: str
    operation: str
    slot: int | None
    generation: int | None
    path: str | None
    monotonic_ns: int


@dataclass
class History:
    path: Path
    operations: int = 0

    @contextmanager
    def operation(
        self, name: str, leaf: Leaf | None = None, path: str | None = None
    ) -> Iterator[None]:
        """Record paired invocations and completions outside the measured fleet."""
        self.operations += 1
        self.write(phase="invoke", name=name, leaf=leaf, path=path)
        try:
            yield
        except BaseException:
            self.write(phase="fail", name=name, leaf=leaf, path=path)
            raise
        self.write(phase="ok", name=name, leaf=leaf, path=path)

    def write(self, phase: str, name: str, leaf: Leaf | None, path: str | None) -> None:
        event = HistoryEvent(
            sequence=self.operations,
            phase=phase,
            operation=name,
            slot=leaf.slot if leaf else None,
            generation=leaf.generation if leaf else None,
            path=path,
            monotonic_ns=time.monotonic_ns(),
        )
        with self.path.open("a", encoding="utf-8") as dst:
            dst.write(json.dumps(asdict(event), sort_keys=True) + "\n")


@dataclass
class Fleet:
    config: FleetConfig
    active: dict[int, Leaf] = field(default_factory=dict, init=False)
    generations: dict[int, int] = field(default_factory=dict, init=False)
    inventory: dict[str, FileState] = field(default_factory=dict, init=False)
    commit: str = field(init=False)
    eligible: tuple[str, ...] = field(init=False)
    rng: random.Random = field(init=False)
    history: History = field(init=False)

    def __post_init__(self) -> None:
        cfg = self.config
        if cfg.root.exists():
            raise ValueError(f"fleet root must not exist: {cfg.root}")
        if cfg.history.exists():
            raise ValueError(f"history must not exist: {cfg.history}")
        self.rng = random.Random(cfg.seed)  # noqa: S311 -- Repeatable workload, not a secret.
        self.commit = self.git(cfg.repo, ["rev-parse", "HEAD"]).decode().strip()
        self.check_status(root=cfg.repo, changed=set())
        for record in self.git(cfg.repo, ["ls-tree", "-rz", "HEAD"]).split(b"\0"):
            if not record:
                continue
            metadata, raw = record.split(b"\t", 1)
            if metadata.split()[1] != b"blob":
                raise OracleError(f"unsupported Git entry: {os.fsdecode(raw)}")
            name = os.fsdecode(raw)
            self.inventory[name] = FileState.read(path=cfg.repo / name)
        self.eligible = tuple(
            sorted(
                name
                for name, entry in self.inventory.items()
                if not entry.symlink and Path(name).suffix in (".c", ".h")
            )
        )
        if not self.eligible:
            raise ValueError("workload requires tracked .c or .h source files")
        cfg.root.mkdir(parents=True)
        cfg.history.parent.mkdir(parents=True, exist_ok=True)
        cfg.history.touch(exist_ok=False)
        self.history = History(path=cfg.history)

    def git(self, root: Path, args: list[str]) -> bytes:
        """Execute Git without a shell, preserving arbitrary path bytes."""
        result = subprocess.run(  # noqa: S603 -- The executable and flags are explicit.
            ["git", "-C", str(root), "-c", "core.fsmonitor=false", *args],  # noqa: S607
            check=True,
            capture_output=True,
        )
        return result.stdout

    def populate(self, count: int) -> None:
        """Create the initial detached worktrees in numbered slots."""
        if count <= 0:
            raise ValueError("count must be positive")
        if self.generations:
            raise ValueError("populate can only initialize an empty fleet")
        self.recreate(slots=tuple(range(count)))

    def recreate(self, slots: tuple[int, ...]) -> None:
        """Create a fresh generation for each previously removed slot."""
        for slot in slots:
            if slot in self.active:
                raise ValueError(f"slot is already active: {slot}")
            generation = self.generations.get(slot, -1) + 1
            leaf = Leaf(slot, generation, self.config.root / f"slot-{slot}-gen-{generation}")
            with self.history.operation(name="add", leaf=leaf):
                if self.config.method is Method.GIT:
                    self.git(
                        self.config.repo,
                        [
                            "worktree",
                            "add",
                            "--detach",
                            str(leaf.path),
                            self.commit,
                        ],
                    )
                else:
                    add_worktree(
                        request=WorktreeAddRequest(
                            path=leaf.path,
                            source=self.config.repo,
                            commitish=self.commit,
                            detach=True,
                        )
                    )
                self.active[slot] = leaf
                self.generations[slot] = generation

    def mutate(self, config: MutationConfig) -> MutationStats:
        """Append valid C comments to the same seeded sample in each arm.

        The fraction applies to tracked regular .c/.h files. Atomic saves rewrite
        the selected whole file with os.replace; in-place appends write only the
        new comment. The oracle computes expected bytes before doing either write.

        """
        count = max(1, math.ceil(len(self.eligible) * config.fraction))
        selection = hashlib.sha256()
        files = written = selected = 0
        for slot in sorted(self.active):
            leaf = self.active[slot]
            for name in sorted(self.rng.sample(self.eligible, count)):
                marker = f"\n/* cowtree workload {config.round_id}:{slot}:{leaf.generation} */\n"
                suffix = marker.encode("ascii")
                selection.update(os.fsencode(f"{slot}:{leaf.generation}:{name}\0"))
                with self.history.operation(name=config.mode.value, leaf=leaf, path=name):
                    written += self.edit(leaf=leaf, name=name, suffix=suffix, mode=config.mode)
                selected += self.inventory[name].size
                files += 1
        result = MutationStats(files, written, selected, selection.hexdigest())
        return result

    def edit(self, leaf: Leaf, name: str, suffix: bytes, mode: EditMode) -> int:
        path = leaf.path / name
        expected = leaf.changed.get(name, self.inventory[name])
        self.check_file(path=path, expected=expected)
        buf = path.read_bytes()
        if b"\0" in buf:
            raise OracleError(f"selected source contains NUL: {path}")
        modified = buf + suffix
        if mode is EditMode.ATOMIC_SAVE:
            tmp = path.with_name(path.name + ".cowtree-save")
            with tmp.open("xb") as dst:
                dst.write(modified)
                dst.flush()
                os.fchmod(dst.fileno(), expected.mode)
                os.fsync(dst.fileno())
            os.replace(tmp, path)
            written = len(modified)
        else:
            with path.open("ab") as dst:
                dst.write(suffix)
                dst.flush()
                os.fsync(dst.fileno())
            written = len(suffix)
        leaf.changed[name] = FileState(
            expected.mode,
            len(modified),
            hashlib.sha256(modified).hexdigest(),
            False,
        )
        return written

    def remove_random(self, count: int, *, external_loss: bool = False) -> tuple[int, ...]:
        """Remove sampled leaves; optionally delete one directory before API cleanup."""
        if not 0 <= count <= len(self.active):
            raise ValueError("removal count exceeds active fleet")
        slots = tuple(sorted(self.rng.sample(sorted(self.active), count)))
        for index, slot in enumerate(slots):
            leaf = self.active[slot]
            if external_loss and index == 0:
                with self.history.operation(name="directory-loss", leaf=leaf):
                    shutil.rmtree(leaf.path)
                    registrations = list_all_worktrees(source=self.config.repo)
                    missing = [tree for tree in registrations if tree.path == leaf.path.resolve()]
                    if len(missing) != 1:
                        raise OracleError(f"missing worktree registration disappeared: {leaf.path}")
                    if not missing[0].prunable:
                        raise OracleError(
                            f"missing worktree is not registered as prunable: {leaf.path}"
                        )
            with self.history.operation(name="remove", leaf=leaf):
                if self.config.method is Method.GIT:
                    self.git(self.config.repo, ["worktree", "remove", "--force", str(leaf.path)])
                else:
                    remove_worktree(path=leaf.path, source=self.config.repo, force=True)
                del self.active[slot]
                if leaf.path.exists():
                    raise OracleError(f"removed worktree still exists: {leaf.path}")
        return slots

    def check_file(self, path: Path, expected: FileState) -> None:
        actual = FileState.read(path=path)
        if actual != expected:
            raise OracleError(f"file differs from oracle: {path}; {actual} != {expected}")

    def check_status(self, root: Path, changed: set[str]) -> None:
        raw = self.git(root, ["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        actual: set[str] = set()
        for record in raw.split(b"\0"):
            if not record:
                continue
            if record[:3] != b" M ":
                raise OracleError(f"unexpected Git status in {root}: {record!r}")
            actual.add(os.fsdecode(record[3:]))
        if actual != changed:
            raise OracleError(f"dirty paths differ in {root}: {actual ^ changed}")

    def check_registry(self) -> None:
        trees = list_all_worktrees(source=self.config.repo)
        expected = {
            self.config.repo.resolve(),
            *(leaf.path.resolve() for leaf in self.active.values()),
        }
        if {tree.path for tree in trees} != expected:
            raise OracleError("Git worktree registry differs from active fleet")
        for tree in trees:
            if tree.path == self.config.repo.resolve():
                continue
            if tree.prunable or tree.locked:
                raise OracleError(f"active worktree has unexpected registry flags: {tree.path}")
            if tree.head != self.commit or not tree.detached:
                raise OracleError(f"active worktree is not detached at pinned commit: {tree.path}")
        expected_dirs = {leaf.path.name for leaf in self.active.values()}
        if {path.name for path in self.config.root.iterdir()} != expected_dirs:
            raise OracleError("fleet root contains an unexpected or missing worktree")

    def verify(self, *, full: bool = True) -> VerificationStats:
        """Check source isolation, survivors, Git state, and the exact active registry."""
        hashed = size = logical = changed = 0
        digest = hashlib.sha256()
        sample = set(sorted(self.inventory)[:: max(1, len(self.inventory) // 128)])
        for leaf in self.active.values():
            sample.update(leaf.changed)
        leaves = [Leaf(-1, 0, self.config.repo), *[self.active[key] for key in sorted(self.active)]]
        with self.history.operation(name="verify-full" if full else "verify-sampled"):
            self.check_registry()
            for leaf in leaves:
                self.check_status(root=leaf.path, changed=set(leaf.changed))
                if self.git(leaf.path, ["rev-parse", "HEAD"]).decode().strip() != self.commit:
                    raise OracleError(f"HEAD changed: {leaf.path}")
                if full:
                    self.check_paths(root=leaf.path)
                names = sorted(self.inventory if full else sample | leaf.changed.keys())
                for name in names:
                    expected = leaf.changed.get(name, self.inventory[name])
                    self.check_file(path=leaf.path / name, expected=expected)
                    hashed += 1
                    size += expected.size
                digest.update(f"{leaf.slot}:{leaf.generation}\0".encode())
                for name, original in sorted(self.inventory.items()):
                    expected = leaf.changed.get(name, original)
                    digest.update(os.fsencode(name) + b"\0" + expected.sha256.encode())
                    digest.update(f":{expected.mode}:{expected.symlink}\0".encode())
                    if leaf.slot >= 0:
                        logical += expected.size
                changed += len(leaf.changed)
        result = VerificationStats(
            full,
            len(self.active),
            hashed,
            size,
            logical,
            changed,
            digest.hexdigest(),
            self.history.operations,
        )
        return result

    def check_paths(self, root: Path) -> None:
        """Detect extra files, including ignored files Git status does not report."""
        actual: set[str] = set()
        for directory, dirs, files in os.walk(root):
            base = Path(directory)
            if base == root:
                dirs[:] = [name for name in dirs if name != ".git"]
                files = [name for name in files if name != ".git"]
            for name in [*dirs, *files]:
                path = base / name
                if path.is_symlink() or name in files:
                    actual.add(str(path.relative_to(root)))
        if actual != self.inventory.keys():
            raise OracleError(
                f"filesystem paths differ in {root}: {actual ^ self.inventory.keys()}"
            )

    def cleanup(self) -> None:
        """Remove every active generation through its implementation's API."""
        self.remove_random(count=len(self.active))
        self.verify(full=True)
