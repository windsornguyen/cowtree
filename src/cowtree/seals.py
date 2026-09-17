"""Retained private checkpoints whose creation can be completed after interruption."""

from dataclasses import dataclass
import hashlib
from pathlib import Path
import shutil
import uuid

from cowtree.durable import sync_directory, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata
from cowtree.metadata_types import Digest, Record
from cowtree.nodes import Nodes
from cowtree.projection import GitProjection
from cowtree.views import Views
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf, Node


class SealRecord(Record):
    before: Leaf
    node: Digest
    retained: bool


class RetainedNode(Record):
    node: Digest


@dataclass(frozen=True)
class Seals:
    workspace: Workspace

    def nodes(self, metadata: Metadata) -> Nodes:
        repository = GitRepository(
            io=CommandRunner(descriptors=metadata.descriptors),
            path=self.workspace.config.git_directory,
        )
        result = Nodes(
            directory=self.workspace.root / "nodes",
            projection=GitProjection(repository),
            policy=self.workspace.config.policy,
        )
        return result

    def seal(self, identity: int, *, retain: bool = True) -> Node:
        with self.workspace.session() as metadata:
            Views(self.workspace).recover_locked(metadata=metadata)
            self.recover_locked(metadata=metadata)
            leaf = Leaves(self.workspace).read(identity=identity)
            Views(self.workspace).current(leaf=leaf)
            if retain and len(list((self.workspace.root / "retained").glob("*.json"))) >= 128:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS,
                    "retained snapshot limit reached; release snapshots",
                )
            record = SealRecord(
                before=leaf, node=hashlib.sha256(uuid.uuid4().bytes).hexdigest(), retained=retain
            )
            directory = self.workspace.root / "operations" / f"seal-{record.node}"
            directory.mkdir(mode=0o700)
            write_record(path=directory / "seal.json", record=record)
            node = self.nodes(metadata=metadata).seal(
                source=leaf.path, origins=leaf.origins, parent=leaf.node, identity=record.node
            )
            self.complete(metadata=metadata, directory=directory, record=record, node=node)
        return node

    def complete(self, metadata: Metadata, directory: Path, record: SealRecord, node: Node) -> None:
        self.nodes(metadata=metadata).verify(node=node)
        if node.parent != record.before.node:
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "checkpoint parent mismatch")
        updated = record.before.model_copy(update={"node": node.id, "git_head": node.git_commit})
        current = Leaves(self.workspace).read(identity=record.before.id)
        if current not in (record.before, updated):
            raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "checkpoint leaf changed")
        repository = self.nodes(metadata=metadata).projection.repository
        repository.run(args=["check-ref-format", f"refs/cowtree/nodes/{node.id}"])
        self.nodes(metadata=metadata).projection.set_ref(
            reference=f"refs/cowtree/nodes/{node.id}", commit=node.git_commit
        )
        with repository.lock() as descriptor:
            target = GitRepository(
                io=CommandRunner(descriptors=(*metadata.descriptors, descriptor)), path=current.path
            )
            target.check_detached(expected=(record.before.git_head, node.git_commit))
            target.run(args=["-c", "core.fsync=all", "reset", "--mixed", "-q", node.git_commit])
            Leaves(self.workspace).save(leaf=updated)
        if record.retained:
            write_record(
                path=self.workspace.root / "retained" / f"{node.id}.json",
                record=RetainedNode(node=node.id),
            )
        shutil.rmtree(directory)
        sync_directory(path=directory.parent)

    def retain(self, identity: str) -> None:
        with self.workspace.session():
            self.workspace.nodes.read(identity=identity)
            path = self.workspace.root / "retained" / f"{identity}.json"
            if not path.exists() and len(list(path.parent.glob("*.json"))) >= 128:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "retained snapshot limit reached"
                )
            write_record(path=path, record=RetainedNode(node=identity))

    def release(self, identity: str) -> None:
        with self.workspace.session():
            self.workspace.nodes.read(identity=identity)
            path = self.workspace.root / "retained" / f"{identity}.json"
            path.unlink(missing_ok=True)
            sync_directory(path=path.parent)

    def recover_locked(self, metadata: Metadata) -> None:
        for directory in sorted((self.workspace.root / "operations").glob("seal-*")):
            if not (directory / "seal.json").exists():
                shutil.rmtree(directory)
                continue
            record = SealRecord.model_validate_json((directory / "seal.json").read_bytes())
            path = self.workspace.root / "nodes" / record.node
            if not (path / "node.json").exists():
                if path.exists():
                    shutil.rmtree(path)
                shutil.rmtree(directory)
                sync_directory(path=self.workspace.root / "nodes")
                sync_directory(path=directory.parent)
                continue
            node = self.workspace.nodes.read(identity=record.node)
            self.complete(metadata=metadata, directory=directory, record=record, node=node)
