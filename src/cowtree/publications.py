"""Commit only a checked candidate and recover its exact acknowledgement."""

from dataclasses import dataclass

from pydantic import TypeAdapter

from cowtree.checks import Checks
from cowtree.durable import write_record
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata
from cowtree.metadata_types import Candidate, Grant, Operation, Receipt, Request
from cowtree.projection import GitProjection
from cowtree.views import Views
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf, PublicationRecord


@dataclass(frozen=True)
class Publications:
    workspace: Workspace

    def prepare(self, identity: int) -> Candidate:
        with self.workspace.session() as metadata:
            leaf = Leaves(self.workspace).read(identity=identity)
            pending = leaf.pending
            if pending is None or not pending.submitted or pending.aborting:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "capture a source request first"
                )
            candidate = metadata.call(
                Operation.PREPARE, {"request": pending.request.model_dump(mode="json")}
            ).decode("candidate", TypeAdapter(Candidate))
            current = Workspace.open(root=self.workspace.root)
            parent = current.nodes.read(identity=current.config.warm_tip)
            published = Views.snapshot(metadata=metadata)
            if published.tip.version != candidate.parent or published.entries != parent.source:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "candidate parent needs projection recovery"
                )
            updated = pending.model_copy(
                update={
                    "candidate": candidate,
                    "batch": None,
                    "parent_node": parent.id,
                    "validation": None,
                }
            )
            Leaves(self.workspace).save(leaf=leaf.model_copy(update={"pending": updated}))
        return candidate

    def commit(self, candidate: Candidate) -> Receipt:
        with self.workspace.session() as metadata:
            leaf = Leaves(self.workspace).read(identity=candidate.request.leaf)
            if leaf.pending is None:
                if leaf.last_receipt is None or leaf.last_receipt.request != candidate.request:
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS, "candidate is not pending"
                    )
                receipt = metadata.call(
                    Operation.COMMIT, {"candidate": candidate.model_dump(mode="json")}
                ).decode("receipt", TypeAdapter(Receipt))
                return receipt
            pending = leaf.pending
            if (
                pending.aborting
                or pending.candidate != candidate
                or pending.validation is None
                or pending.validation.candidate != candidate
            ):
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "exact candidate has not passed validation"
                )
            node = self.workspace.nodes.read(identity=pending.validation.node)
            self.workspace.nodes.verify(node=node)
            if node.source != Checks(self.workspace).manifest(candidate=candidate):
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "validated source differs from candidate"
                )
            receipt = metadata.call(
                Operation.COMMIT, {"candidate": candidate.model_dump(mode="json")}
            ).decode("receipt", TypeAdapter(Receipt))
            self.finish(metadata=metadata, leaf=leaf, receipt=receipt)
        return receipt

    def finish(self, metadata: Metadata, leaf: Leaf, receipt: Receipt) -> None:
        pending = leaf.pending
        assert pending is not None
        if pending.validation is None or pending.candidate is None or pending.parent_node is None:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "committed request lacks its validation record"
            )
        if receipt.request != pending.request or receipt.root != pending.candidate.root:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "receipt differs from pending publication"
            )
        published = Views.snapshot(metadata=metadata)
        if published.tip.version != receipt.version:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "publication history advanced before recovery"
            )
        node = self.workspace.nodes.read(identity=pending.validation.node)
        if node.source != published.entries:
            raise CowtreeError(
                CowtreeErrorCode.COMMAND_FAILED, "published source differs from validated node"
            )
        parent = self.workspace.nodes.read(identity=pending.parent_node)
        repository = GitRepository(
            io=CommandRunner(descriptors=metadata.descriptors),
            path=self.workspace.config.git_directory,
        )
        projection = GitProjection(repository)
        projection.set_ref(
            reference=f"refs/cowtree/tips/{self.workspace.config.initial}",
            commit=node.git_commit,
            expected=parent.git_commit,
        )
        receipts = ()
        if pending.batch is not None:
            receipts = tuple(
                metadata.call(
                    Operation.RESULT, {"request": member.request.model_dump(mode="json")}
                ).decode("result", TypeAdapter(Receipt))
                for member in pending.batch.members
            )
            if any(
                item.version != receipt.version or item.root != receipt.root for item in receipts
            ):
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "batch receipt mismatch")
        current = Workspace.open(root=self.workspace.root)
        write_record(
            path=self.workspace.root / "receipts" / f"{receipt.version}.json",
            record=PublicationRecord(
                receipt=receipt, validation=pending.validation, receipts=receipts
            ),
        )
        write_record(
            path=self.workspace.root / "workspace.json",
            record=current.config.model_copy(update={"warm_tip": node.id}),
        )
        origins = dict(leaf.origins)
        for path, value in pending.changes.items():
            if value is None:
                origins.pop(path, None)
            else:
                origins[path] = value
        grants = metadata.call(Operation.GRANTS).decode("grants", TypeAdapter(list[Grant]))
        updated = leaf.model_copy(
            update={
                "origins": origins,
                "grants": {grant.path: grant for grant in grants if grant.leaf == leaf.id},
                "sequence": pending.request.sequence + 1,
                "pending": None,
                "last_receipt": receipt,
            }
        )
        Leaves(self.workspace).save(leaf=updated)

    def result(self, request: Request) -> Receipt | None:
        with self.workspace.session() as metadata:
            result = metadata.call(
                Operation.RESULT, {"request": request.model_dump(mode="json")}
            ).decode("result", TypeAdapter(Receipt | None))
        return result

    def recover_locked(self, metadata: Metadata) -> None:
        for leaf in Leaves(self.workspace).records():
            pending = leaf.pending
            if pending is None or not pending.submitted or pending.aborting:
                continue
            receipt = metadata.call(
                Operation.RESULT, {"request": pending.request.model_dump(mode="json")}
            ).decode("result", TypeAdapter(Receipt | None))
            if receipt is not None:
                self.finish(metadata=metadata, leaf=leaf, receipt=receipt)
