"""Freeze private source intent once before submitting it to the authority."""

from dataclasses import dataclass
import os

from pydantic import TypeAdapter

from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata, MetadataError
from cowtree.metadata_types import EntryKind, MetadataReason, Operation, Proposal, Receipt, Request
from cowtree.seals import Seals
from cowtree.views import Views
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf, Publication


@dataclass(frozen=True)
class Captures:
    workspace: Workspace

    def capture(self, identity: int) -> Publication | None:
        with self.workspace.session():
            leaf = Leaves(self.workspace).read(identity=identity)
            if leaf.check_candidate is not None:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "check views cannot publish")
            if leaf.pending is not None:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS,
                    "publication already pending; resume or abort it",
                )
            if Views(self.workspace).current(leaf=leaf) == leaf.origins:
                return None
        node = Seals(self.workspace).seal(identity=identity, retain=False)
        leaf = Leaves(self.workspace).read(identity=identity)
        paths = tuple(
            sorted(
                path
                for path in set(node.source) | set(leaf.origins)
                if node.source.get(path) != leaf.origins.get(path)
            )
        )
        Views(self.workspace).acquire(identity=identity, paths=paths)
        with self.workspace.session() as metadata:
            leaf = Leaves(self.workspace).read(identity=identity)
            if leaf.node != node.id or leaf.pending is not None:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "leaf changed during capture"
                )
            changes = {
                path: node.source.get(path)
                for path in paths
                if node.source.get(path) != leaf.origins.get(path)
            }
            if not changes:
                return None
            pending = Publication(
                request=Request(leaf=identity, sequence=leaf.sequence),
                node=node.id,
                changes=changes,
            )
            leaf = leaf.model_copy(update={"pending": pending})
            Leaves(self.workspace).save(leaf=leaf)
            result = self.submit(metadata=metadata, leaf=leaf)
        return result

    def submit(self, metadata: Metadata, leaf: Leaf) -> Publication:
        pending = leaf.pending
        assert pending is not None
        node = self.workspace.nodes.read(identity=pending.node)
        self.workspace.nodes.verify(node=node)
        for path, entry in pending.changes.items():
            grant = leaf.grants[path]
            if entry is not None:
                source = self.workspace.root / "nodes" / node.id / "tree" / path
                if entry.kind is EntryKind.SYMLINK:
                    staged = metadata.call(
                        Operation.STAGE,
                        {"leaf": leaf.id, "data": list(os.fsencode(os.readlink(source)))},
                    )
                else:
                    staged = metadata.call(
                        Operation.STAGE_FILE, {"leaf": leaf.id, "path": str(source)}
                    )
                if staged.decode("object", TypeAdapter(str)) != entry.object:
                    raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "captured object changed")
            metadata.call(
                Operation.EDIT,
                {
                    "grant": grant.model_dump(mode="json"),
                    "value": None if entry is None else entry.model_dump(mode="json"),
                },
            )
        proposal = metadata.call(
            Operation.PROPOSE,
            {
                "input": {
                    "request": pending.request.model_dump(mode="json"),
                    "paths": sorted(pending.changes),
                }
            },
        ).decode("proposal", TypeAdapter(Proposal))
        if {change.path: change.value for change in proposal.changes} != pending.changes:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "request identity disagrees with captured source"
            )
        submitted = pending.model_copy(update={"submitted": True})
        Leaves(self.workspace).save(leaf=leaf.model_copy(update={"pending": submitted}))
        return submitted

    def abort(self, identity: int) -> None:
        with self.workspace.session(recover=False) as metadata:
            Views(self.workspace).recover_locked(metadata=metadata)
            Seals(self.workspace).recover_locked(metadata=metadata)
            Leaves(self.workspace).recover_locked(metadata=metadata)
            leaf = Leaves(self.workspace).read(identity=identity)
            pending = leaf.pending
            if pending is None:
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "no publication is pending")
            if not pending.aborting:
                try:
                    receipt = metadata.call(
                        Operation.RESULT, {"request": pending.request.model_dump(mode="json")}
                    ).decode("result", TypeAdapter(Receipt | None))
                except MetadataError as error:
                    if error.reason is not MetadataReason.REQUEST_EXPIRED or pending.submitted:
                        raise
                    receipt = None
                if receipt is not None:
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS,
                        "publication already committed; recover its receipt",
                    )
                pending = pending.model_copy(update={"aborting": True})
                leaf = leaf.model_copy(update={"pending": pending})
                Leaves(self.workspace).save(leaf=leaf)
            self.cancel(metadata=metadata, leaf=leaf)

    def cancel(self, metadata: Metadata, leaf: Leaf) -> None:
        pending = leaf.pending
        assert pending is not None
        assert pending.aborting
        try:
            metadata.call(Operation.ABORT, {"request": pending.request.model_dump(mode="json")})
        except MetadataError as error:
            if error.reason is not MetadataReason.REQUEST_EXPIRED:
                raise
        for entry in pending.changes.values():
            if entry is not None:
                metadata.call(Operation.DISCARD_UPLOAD, {"leaf": leaf.id, "object": entry.object})
        Leaves(self.workspace).save(
            leaf=leaf.model_copy(update={"pending": None, "sequence": pending.request.sequence + 1})
        )

    def recover_locked(self, metadata: Metadata) -> None:
        for leaf in Leaves(self.workspace).records():
            if leaf.pending is not None and leaf.pending.aborting:
                self.cancel(metadata=metadata, leaf=leaf)
            elif leaf.pending is not None and not leaf.pending.submitted:
                self.submit(metadata=metadata, leaf=leaf)
