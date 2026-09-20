"""Warm managed worktrees with durable ownership and recoverable initialization."""

from __future__ import annotations

from contextlib import ExitStack
from dataclasses import dataclass
import fcntl
from itertools import chain
import os
from pathlib import Path
import shutil
import stat
import uuid

from pydantic import TypeAdapter

from cowtree.durable import sync_directory, sync_tree, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.metadata import Metadata
from cowtree.metadata_types import Candidate, Digest, Operation, Record
from cowtree.nodes import source_manifest
from cowtree.publication import publish_directory
from cowtree.tree_types import CaptureMode, TreeKind
from cowtree.trees import populate_tree, scan_tree
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf, Node


class ForkRecord(Record):
    path: Path
    node: Digest
    before: list[int]
    device: int
    inode: int
    leaf: int | None = None
    check_candidate: Candidate | None = None


@dataclass(frozen=True)
class Leaves:
    workspace: Workspace

    def read(self, identity: int) -> Leaf:
        if identity <= 0:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid leaf identity")
        leaf = Leaf.model_validate_json(
            (self.workspace.root / "leaves" / f"{identity}.json").read_bytes()
        )
        if leaf.id != identity:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "leaf record identity mismatch")
        return leaf

    def records(self) -> list[Leaf]:
        result = [
            Leaf.model_validate_json(path.read_bytes())
            for path in sorted((self.workspace.root / "leaves").glob("*.json"))
        ]
        return result

    def save(self, leaf: Leaf) -> None:
        write_record(path=self.workspace.root / "leaves" / f"{leaf.id}.json", record=leaf)

    def repository(self, descriptors: tuple[int, ...]) -> GitRepository:
        result = GitRepository(
            io=CommandRunner(descriptors=descriptors), path=self.workspace.config.git_directory
        )
        return result

    def target(self, path: Path) -> Path:
        if os.path.lexists(path):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, f"destination exists: {path}")
        target = path.resolve()
        if not target.parent.is_dir():
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "destination parent must exist")
        if target.parent.stat().st_dev != self.workspace.root.stat().st_dev:
            raise CowtreeError(
                CowtreeErrorCode.DIFFERENT_FILESYSTEM, "leaf must share the workspace filesystem"
            )
        roots = [
            self.workspace.root,
            *[tree.path for tree in self.workspace.repository.worktrees()],
        ]
        for parent in target.parents:
            for root in roots:
                if root.exists() and parent.samefile(root):
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS,
                        "leaf cannot nest in another worktree or its store",
                    )
        return target

    def fork(
        self, path: Path, node: str | None = None, check_candidate: Candidate | None = None
    ) -> Leaf:
        """Pin an immutable source while cloning outside the workspace lock."""
        with ExitStack() as stack:
            with self.workspace.session() as metadata:
                self.recover_locked(metadata=metadata)
                target = self.target(path=path)
                current = Workspace.open(root=self.workspace.root)
                snapshot = current.nodes.read(
                    identity=current.config.warm_tip if node is None else node
                )
                directory = self.workspace.root / "operations" / f"fork-{uuid.uuid4().hex}"
                directory.mkdir(mode=0o700)
                lock = stack.enter_context((directory / "lock").open("a+b"))
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
                record = self.prepare(metadata, target, snapshot, directory, lock.fileno())
                record = record.model_copy(update={"check_candidate": check_candidate})
                write_record(path=directory / "fork.json", record=record)
            populate_tree(
                source=self.workspace.root / "nodes" / snapshot.id / "tree",
                target=target,
                policy=snapshot.policy,
                capture=CaptureMode.METADATA,
            )
            with self.workspace.session() as metadata:
                leaf = self.finish(metadata, record, snapshot, lock.fileno())
                shutil.rmtree(directory)
                sync_directory(path=directory.parent)
            return leaf

    def prepare(
        self, metadata: Metadata, target: Path, node: Node, directory: Path, descriptor: int
    ) -> ForkRecord:
        reserved = directory / "reserved"
        reserved.mkdir()
        identity = reserved.stat()
        record = ForkRecord(
            path=target,
            node=node.id,
            before=metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int])),
            device=identity.st_dev,
            inode=identity.st_ino,
        )
        write_record(path=directory / "fork.json", record=record)
        leaf = metadata.call(Operation.CREATE_LEAF).decode("leaf", TypeAdapter(int))
        record = record.model_copy(update={"leaf": leaf})
        write_record(path=directory / "fork.json", record=record)
        publish_directory(source=reserved, target=target)
        repository = self.repository(descriptors=(*metadata.descriptors, descriptor))
        with repository.lock() as git_lock:
            repository = self.repository(descriptors=(*metadata.descriptors, descriptor, git_lock))
            repository.run(
                args=[
                    "-c",
                    "core.fsync=all",
                    "worktree",
                    "add",
                    "--no-checkout",
                    "--detach",
                    "--lock",
                    "--reason",
                    f"cowtree managed leaf {leaf}",
                    "--",
                    str(target),
                    node.git_commit,
                ]
            )
        return record

    def finish(self, metadata: Metadata, record: ForkRecord, node: Node, descriptor: int) -> Leaf:
        self.check_identity(record=record)
        assert record.leaf is not None
        if record.leaf not in metadata.call(Operation.LEAVES).decode(
            "leaves", TypeAdapter(list[int])
        ):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "initializing leaf was dropped")
        entries = scan_tree(root=record.path, policy=node.policy)
        if source_manifest(root=record.path, entries=entries) != node.source:
            raise CowtreeError(
                CowtreeErrorCode.DIRTY_SOURCE, "leaf differs from its source snapshot"
            )
        repository = self.repository(descriptors=(*metadata.descriptors, descriptor))
        with repository.lock() as git_lock:
            runner = CommandRunner(descriptors=(*metadata.descriptors, descriptor, git_lock))
            target = GitRepository(io=runner, path=record.path)
            target.check_detached(expected=(node.git_commit,))
            target.run(args=["-c", "core.fsync=all", "reset", "--mixed", "-q", node.git_commit])
            if target.head() != node.git_commit or target.status():
                raise CowtreeError(CowtreeErrorCode.DIRTY_SOURCE, "new leaf checkout is not clean")
            sync_tree(
                root=record.path.parent,
                files=chain(
                    (record.path / entry.path for entry in entries if entry.kind is TreeKind.FILE),
                    (record.path / ".git",),
                ),
                directories=chain(
                    (
                        record.path / entry.path
                        for entry in reversed(entries)
                        if entry.kind is TreeKind.DIRECTORY
                    ),
                    (record.path,),
                ),
            )
            leaf = Leaf(
                id=record.leaf,
                path=record.path,
                device=record.device,
                inode=record.inode,
                node=node.id,
                git_head=node.git_commit,
                origins=node.origins,
                check_candidate=record.check_candidate,
            )
            self.save(leaf=leaf)
        return leaf

    @staticmethod
    def check_identity(record: ForkRecord) -> None:
        identity = record.path.lstat()
        if not stat.S_ISDIR(identity.st_mode) or (identity.st_dev, identity.st_ino) != (
            record.device,
            record.inode,
        ):
            raise CowtreeError(
                CowtreeErrorCode.INVALID_ARGUMENTS, "initializing directory identity changed"
            )

    def recover(self) -> list[int]:
        with self.workspace.session(recover=False) as metadata:
            recovered = self.workspace.reconcile(metadata=metadata)
        return recovered

    def recover_locked(self, metadata: Metadata) -> list[int]:
        """Retire abandoned forks; live operation locks keep their files pinned."""
        recovered: list[int] = []
        for directory in sorted((self.workspace.root / "operations").glob("fork-*")):
            with (directory / "lock").open("a+b") as lock:
                try:
                    fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    continue
                if (directory / "fork.json").exists():
                    record = ForkRecord.model_validate_json((directory / "fork.json").read_bytes())
                    leaf = self.abort(metadata, record, lock.fileno())
                    if leaf is not None:
                        recovered.append(leaf)
                shutil.rmtree(directory)
                sync_directory(path=directory.parent)
        return recovered

    def abort(self, metadata: Metadata, record: ForkRecord, descriptor: int) -> int | None:
        active = metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int]))
        leaf = record.leaf
        if leaf is None:
            unknown = set(active) - set(record.before)
            if len(unknown) > 1:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "ambiguous abandoned leaf allocation"
                )
            leaf = next(iter(unknown), None)
        if leaf is not None and (self.workspace.root / "leaves" / f"{leaf}.json").exists():
            ready = self.read(identity=leaf)
            if ready.path != record.path or ready.node != record.node:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "fork record conflicts with ready leaf"
                )
            return leaf
        if os.path.lexists(record.path):
            self.check_identity(record=record)
            repository = self.repository(descriptors=(*metadata.descriptors, descriptor))
            with repository.lock() as git_lock:
                repository = self.repository(
                    descriptors=(*metadata.descriptors, descriptor, git_lock)
                )
                if repository.find_worktree(path=record.path) is not None:
                    repository.run(
                        args=["worktree", "remove", "--force", "--force", "--", str(record.path)]
                    )
                else:
                    shutil.rmtree(record.path)
                sync_directory(path=record.path.parent)
        if leaf is not None and leaf in active:
            metadata.call(Operation.DROP_LEAF, {"leaf": leaf})
        return None
