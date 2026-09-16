"""Managed Git worktrees backed by the local Rust SQLite authority.

Editors must be quiescent during capture, installation, and recovery. Git commits
stay independent of publication epochs; published edits remain ordinary Git changes.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, field
import fcntl
import json
import os
from pathlib import Path
from tempfile import NamedTemporaryFile
from typing import cast

from cowtree.core import add_worktree, remove_worktree
from cowtree.errors import CowtreeError
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository, resolve_path
from cowtree.metadata_client import MetadataClient
from cowtree.types import WorktreeAddRequest
from cowtree.workspace_types import (
    BatchCandidate,
    BatchReceipt,
    Candidate,
    Grant,
    Installation,
    Json,
    LeafView,
    MetadataError,
    Proposal,
    Receipt,
    RequestId,
    RetryAction,
    Tip,
    WireRecord,
    WorkspaceLeaf,
    array,
)


@dataclass(frozen=True)
class Workspace:
    """Own one source checkout and an explicitly selected metadata executable."""

    source: Path
    metadata: MetadataClient
    commit: str
    initial_version: int
    io: CommandRunner = field(default_factory=CommandRunner)

    @classmethod
    def create(cls, source: Path, authority: Path, binary: Path) -> Workspace:
        """Import a clean Git snapshot, preserving an initialization recovery record."""
        io = CommandRunner()
        repository = GitRepository.discover(io=io, source=source)
        metadata = MetadataClient(authority=authority.resolve(), binary=binary.resolve())
        with repository.lock():
            checkout = repository.snapshot()
            metadata.call(
                {"op": "init_workspace", "source": str(repository.path), "commit": checkout.commit},
                "done",
            )
        result = cls.open(source=repository.path, authority=metadata.authority, binary=binary)
        return result

    @classmethod
    def open(cls, source: Path, authority: Path, binary: Path) -> Workspace:
        """Open an authority or resume its recorded interrupted source import."""
        authority = authority.resolve()
        source = source.resolve()
        config = read_config(authority=authority)
        if config.number("format") != 1:
            raise MetadataError("workspace_configuration", "unsupported workspace format")
        if Path(config.text("source")) != source:
            raise MetadataError("workspace_configuration", "source differs from registered source")
        metadata = MetadataClient(authority=authority, binary=binary.resolve())
        version = config.value("initial_version")
        if version is None:
            version = finish_import(metadata=metadata, source=source, config=config)
        else:
            version = config.number("initial_version")
            metadata.call({"op": "tip"}, "tip")
        result = cls(
            source=source,
            metadata=metadata,
            commit=config.text("commit"),
            initial_version=version,
        )
        return result

    def add(self, path: Path, branch: str | None = None) -> WorkspaceLeaf:
        """Clone the pinned source, bind its leaf, then install the published tip.

        On interruption, the Git worktree and leaf are preserved. Use ``bind`` to
        finish registration and ``recover`` for an interrupted installation.

        """
        repository = GitRepository.discover(io=self.io, source=self.source)
        if repository.head() != self.commit:
            raise MetadataError("source_changed", "source HEAD differs from imported commit")
        value = self.metadata.call({"op": "create_leaf"}, "leaf")
        leaf_id = WireRecord(fields={"leaf": value}).number("leaf")
        try:
            worktree = add_worktree(
                request=WorktreeAddRequest(
                    path=path,
                    source=self.source,
                    branch=branch,
                    commitish=self.commit,
                ),
                runner=self.io,
            )
        except BaseException as original:
            try:
                self.metadata.call({"op": "drop_leaf", "leaf": leaf_id}, "done")
            except MetadataError as cleanup:
                raise MetadataError(
                    "workspace_cleanup", f"{original}; dropping leaf {leaf_id} failed: {cleanup}"
                ) from original
            raise
        leaf = WorkspaceLeaf(leaf=leaf_id, path=worktree.path)
        self.bind(leaf)
        self.sync(leaf)
        return leaf

    def bind(self, leaf: WorkspaceLeaf) -> Installation:
        """Bind a source clone to its imported epoch; reject mismatching file contents."""
        value = self.metadata.call(
            {
                "op": "bind_tree",
                "leaf": leaf.leaf,
                "path": str(leaf.path.resolve()),
                "version": self.initial_version,
            },
            "installation",
        )
        result = Installation.parse(value)
        return result

    def binding(self, leaf: int) -> Installation:
        """Read a persisted binding so callers can resume after process loss."""
        result = Installation.parse(
            self.metadata.call({"op": "binding", "leaf": leaf}, "installation")
        )
        return result

    def leaves(self) -> tuple[int, ...]:
        """List active identities, including an interrupted add's unbound identity."""
        value = self.metadata.call({"op": "leaves"}, "leaves")
        result = tuple(WireRecord({"leaf": item}).number("leaf") for item in array(value))
        return result

    def tip(self) -> Tip:
        """Read the latest authoritative publication epoch."""
        result = Tip.parse(self.metadata.call({"op": "tip"}, "tip"))
        return result

    def acquire(self, leaf: WorkspaceLeaf, paths: Sequence[str]) -> tuple[Grant, ...]:
        """Reserve exact paths; stale installed origins require an explicit sync."""
        value = self.metadata.call(
            {"op": "acquire", "leaf": leaf.leaf, "paths": list(paths)},
            "grants",
        )
        result = tuple(Grant.parse(item) for item in array(value))
        return result

    def activate(self, grant: Grant) -> Grant:
        """Activate a fenced reservation whose origin is already installed."""
        value = self.metadata.call({"op": "activate", "grant": grant.wire()}, "grant")
        result = Grant.parse(value)
        return result

    def capture(self, grants: Sequence[Grant]) -> tuple[LeafView, ...]:
        """Capture file bytes, kinds, and deletion intents from the bound worktree."""
        value = self.metadata.call(
            {"op": "capture_files", "grants": [grant.wire() for grant in grants]},
            "views",
        )
        result = tuple(LeafView.parse(item) for item in array(value))
        return result

    def publish(self, request: RequestId, paths: Sequence[str]) -> Receipt:
        """Publish already captured paths; leave installation as an explicit operation."""
        self.propose(request=request, paths=paths)
        candidate = self.prepare(request)
        result = self.commit_candidate(candidate)
        return result

    def propose(self, request: RequestId, paths: Sequence[str]) -> Proposal:
        """Freeze captured edits under an idempotent request identity."""
        value = self.metadata.call(
            {"op": "propose", "input": {"request": request.wire(), "paths": list(paths)}},
            "proposal",
        )
        result = Proposal.parse(value)
        return result

    def prepare(self, request: RequestId) -> Candidate:
        """Prepare a pending capture against the latest tip without changing its edits."""
        value = self.metadata.call({"op": "prepare", "request": request.wire()}, "candidate")
        result = Candidate.parse(value)
        return result

    def commit_candidate(self, candidate: Candidate) -> Receipt:
        """Commit one prepared candidate or return its durable prior receipt."""
        value = self.metadata.call({"op": "commit", "candidate": candidate.wire()}, "receipt")
        result = Receipt.parse(value)
        return result

    def prepare_batch(
        self,
        requests: Sequence[RequestId],
        expected_tip: int,
    ) -> BatchCandidate:
        """Prepare disjoint captures for one atomic epoch at the supplied tip."""
        value = self.metadata.call(
            {
                "op": "prepare_batch",
                "requests": [request.wire() for request in requests],
                "expected_tip": expected_tip,
            },
            "batch_candidate",
        )
        result = BatchCandidate.parse(value)
        return result

    def commit_batch(self, candidate: BatchCandidate) -> BatchReceipt:
        """Commit complete batch membership atomically or recover its prior receipts."""
        value = self.metadata.call(
            {"op": "commit_batch", "candidate": candidate.wire()},
            "batch_receipt",
        )
        result = BatchReceipt.parse(value)
        return result

    def resolve(
        self,
        request: RequestId,
        sources: Sequence[RequestId],
        choices: Mapping[str, RequestId],
    ) -> Proposal:
        """Replace same-owner captures with explicit per-path choices; retain later edits."""
        value = self.metadata.call(
            {
                "op": "resolve",
                "input": {
                    "request": request.wire(),
                    "sources": [source.wire() for source in sources],
                    "choices": {path: selected.wire() for path, selected in choices.items()},
                },
            },
            "proposal",
        )
        result = Proposal.parse(value)
        return result

    def result(self, request: RequestId) -> Receipt | None:
        """Recover an acknowledged or interrupted commit by its stable request ID."""
        value = self.metadata.call({"op": "result", "request": request.wire()}, "result")
        result = None if value is None else Receipt.parse(value)
        return result

    def sync(self, leaf: WorkspaceLeaf, version: int | None = None) -> Installation:
        """Install an epoch; refuse unrecorded local edits and untracked collisions."""
        if version is None:
            version = self.tip().version
        value = self.metadata.call(
            {"op": "install", "leaf": leaf.leaf, "version": version},
            "installation",
        )
        result = Installation.parse(value)
        return result

    def recover(self, leaf: WorkspaceLeaf) -> Installation:
        """Finish a recorded installation after process interruption."""
        value = self.metadata.call({"op": "recover", "leaf": leaf.leaf}, "installation")
        result = Installation.parse(value)
        return result

    def release(self, grant: Grant) -> None:
        """Release exactly the supplied fencing generation."""
        self.metadata.call({"op": "release", "grant": grant.wire()}, "done")

    def remove(self, leaf: WorkspaceLeaf, force: bool = False) -> None:
        """Remove through Git's dirty checks before dropping the metadata identity."""
        try:
            with workspace_lock(authority=self.metadata.authority):
                binding = self.binding(leaf.leaf)
                target = resolve_path(path=leaf.path)
                try:
                    matches = binding.path == target or binding.path.samefile(target)
                except FileNotFoundError:
                    matches = False
                if not matches:
                    raise MetadataError(
                        "binding_changed", "leaf path differs from registered binding"
                    )
                if binding.pending is not None:
                    raise MetadataError(
                        "needs_recovery",
                        "finish installation before removing this worktree",
                        RetryAction.RECOVER_INSTALLATION,
                    )
                repository = GitRepository.discover(io=self.io, source=self.source)
                registered = repository.find_worktree(path=binding.path)
                if registered is not None:
                    remove_worktree(
                        path=registered.path, source=self.source, force=force, runner=self.io
                    )
                self.metadata.call({"op": "drop_leaf", "leaf": leaf.leaf}, "done")
        except CowtreeError as error:
            raise MetadataError(error.code.value, error.message) from error
        except OSError as error:
            raise MetadataError("workspace_io", f"worktree removal failed: {error}") from error


@contextmanager
def workspace_lock(authority: Path) -> Iterator[None]:
    """Acquire the authority lock before Git's lock or SQLite transactions.

    Rust filesystem operations own this same flock. Call only metadata operations
    that do not acquire it while holding it in Python, or the child would deadlock.

    """
    with (authority / "workspace.lock").open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


def finish_import(metadata: MetadataClient, source: Path, config: WireRecord) -> int:
    """Resume only the source commit recorded before import began."""
    repository = GitRepository.discover(io=CommandRunner(), source=source)
    with repository.lock():
        current = read_config(authority=metadata.authority)
        if current.number("format") != config.number("format"):
            raise MetadataError("workspace_configuration", "workspace format changed")
        if current.number("format") != 1:
            raise MetadataError("workspace_configuration", "unsupported workspace format")
        for name in ("source", "commit"):
            if current.text(name) != config.text(name):
                raise MetadataError("workspace_configuration", f"workspace {name} changed")
        if Path(current.text("source")) != repository.path:
            raise MetadataError("workspace_configuration", "source differs from registered source")
        if current.value("initial_version") is not None:
            version = current.number("initial_version")
            metadata.call({"op": "tip"}, "tip")
            return version
        checkout = repository.snapshot()
        if checkout.commit != current.text("commit"):
            raise MetadataError("source_changed", "source changed during workspace initialization")
        value = metadata.call(
            {
                "op": "import_tree",
                "source": str(source),
                "paths": [entry.path for entry in checkout.files],
            },
            "tip",
        )
        tip = Tip.parse(value)
        metadata.call({"op": "retain", "version": tip.version}, "done")
        fields = dict(current.fields)
        fields["initial_version"] = tip.version
        write_config(authority=metadata.authority, config=fields)
    return tip.version


def read_config(authority: Path) -> WireRecord:
    """Read the current atomically published initialization record."""
    try:
        data = cast(Json, json.loads((authority / "workspace.json").read_text()))
    except (OSError, ValueError) as error:
        raise MetadataError("workspace_configuration", str(error)) from error
    result = WireRecord.parse(data)
    return result


def write_config(authority: Path, config: dict[str, Json]) -> None:
    """Publish the recovery record before returning initialization success."""
    target = authority / "workspace.json"
    with NamedTemporaryFile(mode="w", dir=authority, delete=False) as stream:
        pending = Path(stream.name)
        json.dump(config, stream, ensure_ascii=True, allow_nan=False)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(pending, target)
    fd = os.open(authority, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)
