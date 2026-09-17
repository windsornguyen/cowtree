"""One checked union publishes all members while retaining each writer's later edits."""

from pathlib import Path
import sys

from pydantic import JsonValue
import pytest

from cowtree.batches import Batches
from cowtree.captures import Captures
from cowtree.checks import Checks
from cowtree.errors import CowtreeError
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata
from cowtree.metadata_types import BatchCandidate, Operation, Output
from cowtree.publications import Publications
from cowtree.workspace_types import Leaf, PublicationRecord

from .test_fork import manager


def prepared(root: Path) -> tuple[Leaves, BatchCandidate]:
    leaves = manager(root=root)
    left = leaves.fork(path=root / "left")
    right = leaves.fork(path=root / "right")
    (left.path / "file.txt").write_bytes(b"left capture")
    (right.path / "second.txt").write_bytes(b"right capture")
    captures = Captures(leaves.workspace)
    captures.capture(identity=left.id)
    captures.capture(identity=right.id)
    candidate = Batches(leaves.workspace).prepare(identities=(right.id, left.id))
    return leaves, candidate


def test_batch_requires_one_checked_union_and_preserves_later_edits(tmp_path: Path) -> None:
    leaves, candidate = prepared(root=tmp_path)
    batches = Batches(leaves.workspace)
    with pytest.raises(CowtreeError, match="not passed validation"):
        batches.commit(candidate=candidate)
    command = (
        sys.executable,
        "-c",
        "from pathlib import Path; assert Path('file.txt').read_bytes()==b'left capture'; "
        "assert Path('second.txt').read_bytes()==b'right capture'; "
        "Path('cache/artifact').write_bytes(b'checked union')",
    )
    validation = batches.validate(candidate=candidate, command=command)
    left, right = [leaves.read(identity=member.request.leaf) for member in candidate.members]
    (left.path / "file.txt").write_bytes(b"later left")
    (right.path / "second.txt").write_bytes(b"later right")
    result = batches.commit(candidate=candidate)
    assert len(result.receipts) == 2
    assert result.receipts[0].version == result.receipts[1].version
    assert result.receipts[0].root == result.receipts[1].root
    assert batches.commit(candidate=candidate) == result
    assert (left.path / "file.txt").read_bytes() == b"later left"
    assert (right.path / "second.txt").read_bytes() == b"later right"
    reader = leaves.fork(path=tmp_path / "reader")
    assert (reader.path / "file.txt").read_bytes() == b"left capture"
    assert (reader.path / "second.txt").read_bytes() == b"right capture"
    assert (reader.path / "cache/artifact").read_bytes() == b"checked union"
    record = PublicationRecord.model_validate_json(
        (leaves.workspace.root / "receipts" / f"{result.receipts[0].version}.json").read_bytes()
    )
    assert record.receipts == tuple(result.receipts)
    assert record.validation.node == validation.node


def test_repreparing_any_member_invalidates_previous_batch_validation(tmp_path: Path) -> None:
    leaves, candidate = prepared(root=tmp_path)
    batches = Batches(leaves.workspace)
    batches.validate(candidate=candidate, command=(sys.executable, "-c", "pass"))
    Publications(leaves.workspace).prepare(identity=candidate.members[1].request.leaf)
    with pytest.raises(CowtreeError, match="batch is no longer current"):
        batches.commit(candidate=candidate)
    with pytest.raises(CowtreeError, match="batch is no longer current"):
        batches.validate(candidate=candidate, command=(sys.executable, "-c", "pass"))
    replacement = batches.prepare(
        identities=tuple(member.request.leaf for member in candidate.members)
    )
    with pytest.raises(CowtreeError, match="not passed validation"):
        batches.commit(candidate=replacement)


def test_checked_members_cannot_publish_separately_or_with_changed_identity(tmp_path: Path) -> None:
    leaves, candidate = prepared(root=tmp_path)
    batches = Batches(leaves.workspace)
    batches.validate(candidate=candidate, command=(sys.executable, "-c", "pass"))
    with pytest.raises(CowtreeError, match="belongs to a batch"):
        Publications(leaves.workspace).commit(candidate=candidate.members[0])
    partial = candidate.model_copy(update={"members": candidate.members[:1]})
    with pytest.raises(CowtreeError, match="batch is no longer current"):
        batches.commit(candidate=partial)
    changed = candidate.members[0].model_copy(update={"attempt": candidate.members[0].attempt + 1})
    altered = candidate.model_copy(update={"members": [changed, *candidate.members[1:]]})
    with pytest.raises(CowtreeError, match="batch is no longer current"):
        batches.commit(candidate=altered)
    assert Publications(leaves.workspace).result(request=candidate.members[0].request) is None
    assert len(batches.commit(candidate=candidate).receipts) == 2


def test_source_mutating_check_cannot_publish_a_batch(tmp_path: Path) -> None:
    leaves, candidate = prepared(root=tmp_path)
    batches = Batches(leaves.workspace)
    command = (
        sys.executable,
        "-c",
        "from pathlib import Path; Path('second.txt').write_bytes(b'check changed source')",
    )
    with pytest.raises(CowtreeError, match="changed source"):
        batches.validate(candidate=candidate, command=command)
    with pytest.raises(CowtreeError, match="not passed validation"):
        batches.commit(candidate=candidate)


def test_advancing_the_tip_requires_rechecking_the_new_union(tmp_path: Path) -> None:
    leaves, candidate = prepared(root=tmp_path)
    batches = Batches(leaves.workspace)
    batches.validate(candidate=candidate, command=(sys.executable, "-c", "pass"))
    independent = leaves.fork(path=tmp_path / "independent")
    (independent.path / "third.txt").write_bytes(b"independent publication")
    Captures(leaves.workspace).capture(identity=independent.id)
    publications = Publications(leaves.workspace)
    other = publications.prepare(identity=independent.id)
    Checks(leaves.workspace).validate(
        identity=independent.id, command=(sys.executable, "-c", "pass")
    )
    publications.commit(candidate=other)
    with pytest.raises(CowtreeError, match="tip changed"):
        batches.commit(candidate=candidate)
    updated = batches.prepare(identities=tuple(member.request.leaf for member in candidate.members))
    with pytest.raises(CowtreeError, match="not passed validation"):
        batches.commit(candidate=updated)
    batches.validate(candidate=updated, command=(sys.executable, "-c", "pass"))
    batches.commit(candidate=updated)
    reader = leaves.fork(path=tmp_path / "reader")
    assert (reader.path / "file.txt").read_bytes() == b"left capture"
    assert (reader.path / "second.txt").read_bytes() == b"right capture"
    assert (reader.path / "third.txt").read_bytes() == b"independent publication"


def test_interrupted_membership_persistence_can_be_prepared_again(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves, previous = prepared(root=tmp_path)
    batches = Batches(leaves.workspace)
    identities = tuple(member.request.leaf for member in previous.members)
    original_save = Leaves.save

    def interrupt(self: Leaves, leaf: Leaf) -> None:
        if leaf.id == identities[1] and leaf.pending is not None:
            raise InterruptedError("membership persistence interrupted")
        original_save(self, leaf=leaf)

    with monkeypatch.context() as patch:
        patch.setattr(Leaves, "save", interrupt)
        with pytest.raises(InterruptedError):
            batches.prepare(identities=identities)
    with pytest.raises(CowtreeError, match="batch is no longer current"):
        batches.commit(candidate=previous)
    candidate = batches.prepare(identities=identities)
    batches.validate(candidate=candidate, command=(sys.executable, "-c", "pass"))
    assert len(batches.commit(candidate=candidate).receipts) == 2


@pytest.mark.parametrize("boundary", ["reply", "leaf"])
def test_batch_recovery_finishes_every_member_of_the_same_epoch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, boundary: str
) -> None:
    leaves, candidate = prepared(root=tmp_path)
    batches = Batches(leaves.workspace)
    batches.validate(candidate=candidate, command=(sys.executable, "-c", "pass"))
    original_call = Metadata.call
    original_save = Leaves.save

    def interrupt_call(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        output = original_call(self, operation, payload)
        if boundary == "reply" and operation is Operation.COMMIT_BATCH:
            raise InterruptedError("batch reply lost")
        return output

    def interrupt_save(self: Leaves, leaf: Leaf) -> None:
        if (
            boundary == "leaf"
            and leaf.id == candidate.members[1].request.leaf
            and leaf.last_receipt is not None
        ):
            raise InterruptedError("member acknowledgement interrupted")
        original_save(self, leaf=leaf)

    with monkeypatch.context() as patch:
        patch.setattr(Metadata, "call", interrupt_call)
        patch.setattr(Leaves, "save", interrupt_save)
        with pytest.raises(InterruptedError):
            batches.commit(candidate=candidate)
    assert leaves.read(identity=candidate.members[1].request.leaf).pending is not None
    result = batches.commit(candidate=candidate)
    for receipt in result.receipts:
        leaf = leaves.read(identity=receipt.request.leaf)
        assert leaf.pending is None
        assert leaf.last_receipt == receipt
    reader = leaves.fork(path=tmp_path / "recovered")
    assert (reader.path / "file.txt").read_bytes() == b"left capture"
    assert (reader.path / "second.txt").read_bytes() == b"right capture"
