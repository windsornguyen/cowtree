"""Check one union of captured requests before publishing all members atomically."""

from dataclasses import dataclass

from pydantic import TypeAdapter

from cowtree.checks import Checks
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata
from cowtree.metadata_types import BatchCandidate, BatchReceipt, Operation, Receipt
from cowtree.publications import Publications
from cowtree.views import Views
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf, Validation


@dataclass(frozen=True)
class Batches:
    workspace: Workspace

    def prepare(self, identities: tuple[int, ...]) -> BatchCandidate:
        """Prepare disjoint pending requests against the current published warm tip."""
        if not identities or len(set(identities)) != len(identities):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "batch needs distinct leaves")
        with self.workspace.session() as metadata:
            leaves = Leaves(self.workspace)
            selected = [leaves.read(identity=identity) for identity in sorted(identities)]
            requests = []
            for leaf in selected:
                pending = leaf.pending
                if pending is None or not pending.submitted or pending.aborting:
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS, "capture every batch member first"
                    )
                requests.append(pending.request.model_dump(mode="json"))
            current = Workspace.open(root=self.workspace.root)
            parent = current.nodes.read(identity=current.config.warm_tip)
            published = Views.snapshot(metadata=metadata)
            if parent.source != published.entries:
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "batch parent needs projection recovery"
                )
            candidate = metadata.call(
                Operation.PREPARE_BATCH,
                {"requests": requests, "expected_tip": published.tip.version},
            ).decode("batch_candidate", TypeAdapter(BatchCandidate))
            for leaf, member in zip(selected, candidate.members, strict=True):
                assert leaf.pending is not None
                pending = leaf.pending.model_copy(
                    update={
                        "candidate": member,
                        "batch": candidate,
                        "parent_node": parent.id,
                        "validation": None,
                    }
                )
                leaves.save(leaf=leaf.model_copy(update={"pending": pending}))
        return candidate

    def pending(self, candidate: BatchCandidate) -> list[Leaf]:
        """Require the complete current membership before using its shared validation."""
        identities = [member.request.leaf for member in candidate.members]
        if not identities or identities != sorted(set(identities)):
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid batch membership")
        leaves = Leaves(self.workspace)
        selected = [leaves.read(identity=identity) for identity in identities]
        for leaf, member in zip(selected, candidate.members, strict=True):
            pending = leaf.pending
            if (
                pending is None
                or pending.aborting
                or pending.candidate != member
                or pending.batch != candidate
            ):
                raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "batch is no longer current")
        return selected

    def validate(
        self, candidate: BatchCandidate, command: tuple[str, ...], timeout_seconds: int = 300
    ) -> Validation:
        """Run one check on the union and attach the same sealed node to every member."""
        with self.workspace.session():
            self.pending(candidate=candidate)
        checks = Checks(self.workspace)
        validation = checks.validate(
            identity=candidate.members[0].request.leaf,
            command=command,
            timeout_seconds=timeout_seconds,
        )
        with self.workspace.session():
            selected = self.pending(candidate=candidate)
            if validation.candidate != candidate.members[0]:
                raise CowtreeError(
                    CowtreeErrorCode.INVALID_ARGUMENTS, "batch changed during validation"
                )
            leaves = Leaves(self.workspace)
            for leaf, member in zip(selected, candidate.members, strict=True):
                assert leaf.pending is not None
                pending = leaf.pending.model_copy(
                    update={"validation": validation.model_copy(update={"candidate": member})}
                )
                leaves.save(leaf=leaf.model_copy(update={"pending": pending}))
        return validation

    def committed(self, metadata: Metadata, candidate: BatchCandidate) -> bool:
        """Distinguish an acknowledged batch retry from an unchecked prepared batch."""
        if not candidate.members:
            raise CowtreeError(CowtreeErrorCode.INVALID_ARGUMENTS, "invalid batch membership")
        receipts = [
            metadata.call(
                Operation.RESULT, {"request": member.request.model_dump(mode="json")}
            ).decode("result", TypeAdapter(Receipt | None))
            for member in candidate.members
        ]
        if any(receipt is None for receipt in receipts):
            if any(receipt is not None for receipt in receipts):
                raise CowtreeError(CowtreeErrorCode.COMMAND_FAILED, "incomplete batch receipts")
            return False
        return True

    def commit(self, candidate: BatchCandidate) -> BatchReceipt:
        """Publish only an exact checked union, then reconcile each durable receipt."""
        with self.workspace.session() as metadata:
            if self.committed(metadata=metadata, candidate=candidate):
                result = metadata.call(
                    Operation.COMMIT_BATCH, {"candidate": candidate.model_dump(mode="json")}
                ).decode("batch_receipt", TypeAdapter(BatchReceipt))
                return result
            selected = self.pending(candidate=candidate)
            first = selected[0].pending
            assert first is not None
            validation = first.validation
            for leaf, member in zip(selected, candidate.members, strict=True):
                assert leaf.pending is not None
                if validation is None or leaf.pending.validation != validation.model_copy(
                    update={"candidate": member}
                ):
                    raise CowtreeError(
                        CowtreeErrorCode.INVALID_ARGUMENTS, "batch has not passed validation"
                    )
            assert validation is not None
            node = self.workspace.nodes.read(identity=validation.node)
            self.workspace.nodes.verify(node=node)
            if node.source != Checks(self.workspace).manifest(candidate=candidate.members[0]):
                raise CowtreeError(
                    CowtreeErrorCode.COMMAND_FAILED, "validated source differs from batch"
                )
            result = metadata.call(
                Operation.COMMIT_BATCH, {"candidate": candidate.model_dump(mode="json")}
            ).decode("batch_receipt", TypeAdapter(BatchReceipt))
            publications = Publications(self.workspace)
            for leaf, receipt in zip(selected, result.receipts, strict=True):
                publications.finish(metadata=metadata, leaf=leaf, receipt=receipt)
        return result
