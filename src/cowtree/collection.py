"""Collect unreferenced client images without reclaiming live operation inputs."""

from dataclasses import dataclass
from pathlib import Path
import shutil
import uuid

from pydantic import NonNegativeInt, TypeAdapter

from cowtree.durable import sync_directory, write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.leaves import ForkRecord, Leaves
from cowtree.lifecycle import DropRecord, Lifecycle, ValidationBusyError
from cowtree.metadata import Metadata
from cowtree.metadata_types import Digest, Operation, Record
from cowtree.projection import GitProjection
from cowtree.publication import publish_directory
from cowtree.seals import RetainedNode, SealRecord
from cowtree.views import ViewRecord
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf, PublicationRecord


class Maintenance(Record):
    epochs_removed: NonNegativeInt
    receipts_removed: NonNegativeInt
    objects_removed: NonNegativeInt
    temporary_files_removed: NonNegativeInt
    database_bytes: NonNegativeInt
    wal_bytes: NonNegativeInt


class Collection(Record):
    nodes: list[Digest]
    checks: list[int]
    metadata: Maintenance


class QuarantinedNode(Record):
    node: Digest
    commit: str | None


def leaf_roots(leaf: Leaf) -> set[str]:
    roots = {leaf.node}
    if leaf.pending is not None:
        roots.add(leaf.pending.node)
        if leaf.pending.parent_node is not None:
            roots.add(leaf.pending.parent_node)
        if leaf.pending.validation is not None:
            roots.add(leaf.pending.validation.node)
    return roots


@dataclass(frozen=True)
class Collector:
    workspace: Workspace

    def collect(self) -> Collection:
        with self.workspace.session() as metadata:
            checks: list[int] = []
            lifecycle = Lifecycle(self.workspace)
            for leaf in Leaves(self.workspace).records():
                if leaf.check_candidate is None:
                    continue
                try:
                    with lifecycle.guard(leaf=leaf) as descriptors:
                        directory = self.workspace.root / "operations" / f"drop-{uuid.uuid4().hex}"
                        directory.mkdir(mode=0o700)
                        record = DropRecord(leaf=leaf)
                        write_record(path=directory / "drop.json", record=record)
                        lifecycle.remove(
                            metadata=metadata,
                            directory=directory,
                            record=record,
                            descriptors=descriptors,
                        )
                        checks.append(leaf.id)
                except ValidationBusyError:
                    continue
            protected = self.roots()
            removed: list[str] = []
            for directory in sorted((self.workspace.root / "nodes").iterdir()):
                if directory.name in protected:
                    continue
                commit = None
                if (directory / "node.json").exists():
                    commit = self.workspace.nodes.read(identity=directory.name).git_commit
                write_record(
                    path=self.workspace.root / "trash" / f"{directory.name}.json",
                    record=QuarantinedNode(node=directory.name, commit=commit),
                )
                removed.append(directory.name)
            self.recover_locked(metadata=metadata)
            self.artifacts()
            self.workspace.pin_origins(metadata=metadata)
            report = metadata.call(Operation.MAINTAIN).decode(
                "maintenance", TypeAdapter(Maintenance)
            )
            result = Collection(nodes=removed, checks=checks, metadata=report)
        return result

    def recover_locked(self, metadata: Metadata) -> None:
        """Finish interrupted quarantine before a new operation can acquire a node."""
        markers = sorted((self.workspace.root / "trash").glob("*.json"))
        if not markers:
            return
        protected = self.roots()
        projection = GitProjection(
            Leaves(self.workspace).repository(descriptors=metadata.descriptors)
        )
        for marker in markers:
            node = QuarantinedNode.model_validate_json(marker.read_bytes())
            if node.node != marker.stem:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "quarantined node identity mismatch"
                )
            source = self.workspace.root / "nodes" / node.node
            directory = self.workspace.root / "trash" / node.node
            if node.node in protected:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "quarantined node is still referenced"
                )
            if source.exists():
                publish_directory(source=source, target=directory)
            if node.commit is not None:
                projection.remove_ref(
                    reference=f"refs/cowtree/nodes/{node.node}", expected=node.commit
                )
            if directory.exists():
                shutil.rmtree(directory)
                sync_directory(path=directory.parent)
            marker.unlink()
            sync_directory(path=marker.parent)

    def roots(self) -> set[str]:
        current = Workspace.open(root=self.workspace.root)
        roots = {current.config.initial, current.config.warm_tip}
        for leaf in Leaves(self.workspace).records():
            roots.update(leaf_roots(leaf))
        for path in (self.workspace.root / "retained").glob("*.json"):
            retained = RetainedNode.model_validate_json(path.read_bytes())
            if path.stem != retained.node:
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "retention identity mismatch")
            roots.add(retained.node)
        for directory in (self.workspace.root / "operations").iterdir():
            if (directory / "fork.json").exists():
                roots.add(
                    ForkRecord.model_validate_json((directory / "fork.json").read_bytes()).node
                )
            elif (directory / "seal.json").exists():
                record = SealRecord.model_validate_json((directory / "seal.json").read_bytes())
                roots.update({record.node, record.before.node})
            elif (directory / "view.json").exists():
                view = ViewRecord.model_validate_json((directory / "view.json").read_bytes())
                roots.update(leaf_roots(view.before))
                if view.after is not None:
                    roots.update(leaf_roots(view.after))
            else:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "unrecognized operation blocks collection"
                )
        return roots

    def artifacts(self) -> None:
        receipts = sorted(
            (self.workspace.root / "receipts").glob("*.json"),
            key=lambda path: int(path.stem),
            reverse=True,
        )
        protected_logs: set[Path] = set()
        for path in receipts[:128]:
            record = PublicationRecord.model_validate_json(path.read_bytes())
            protected_logs.add(Path(record.validation.log))
        for path in receipts[128:]:
            path.unlink()
        for leaf in Leaves(self.workspace).records():
            if leaf.check_candidate is not None:
                protected_logs.add(self.workspace.root / "checks" / f"{leaf.id}.log")
            if leaf.pending is not None and leaf.pending.validation is not None:
                protected_logs.add(Path(leaf.pending.validation.log))
        logs = sorted(
            (self.workspace.root / "checks").glob("*.log"),
            key=lambda path: path.stat().st_mtime_ns,
            reverse=True,
        )
        for path in logs[64:]:
            if path not in protected_logs:
                path.unlink()
        live = {leaf.id for leaf in Leaves(self.workspace).records()}
        for path in (self.workspace.root / "checks").glob("*-install"):
            if int(path.name.removesuffix("-install")) not in live:
                shutil.rmtree(path)
        sync_directory(path=self.workspace.root / "receipts")
        sync_directory(path=self.workspace.root / "checks")
