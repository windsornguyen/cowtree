"""Frozen requests survive lost replies without consuming subsequent working edits."""

import hashlib
from pathlib import Path

from pydantic import JsonValue, TypeAdapter
import pytest

from cowtree.captures import Captures
from cowtree.metadata import Metadata
from cowtree.metadata_types import Operation, Output, Proposal

from .test_fork import manager


def test_capture_freezes_source_and_abort_preserves_working_files(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    target = leaf.path / "file.txt"
    target.write_bytes(b"captured")
    captures = Captures(leaves.workspace)
    pending = captures.capture(identity=leaf.id)
    assert pending is not None
    assert pending.submitted
    target.write_bytes(b"later edit")
    with leaves.workspace.session() as metadata:
        proposal = metadata.call(
            Operation.PROPOSE,
            {"input": {"request": pending.request.model_dump(mode="json"), "paths": ["file.txt"]}},
        ).decode("proposal", TypeAdapter(Proposal))
    assert proposal.changes[0].value is not None
    assert proposal.changes[0].value.object == hashlib.sha256(b"captured").hexdigest()
    captures.abort(identity=leaf.id)
    assert target.read_bytes() == b"later edit"
    updated = leaves.read(identity=leaf.id)
    assert updated.pending is None
    assert updated.sequence == 2


def test_lost_submission_reply_reuses_the_capture(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    target = leaf.path / "file.txt"
    target.write_bytes(b"captured")
    original = Metadata.call

    def interrupt(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        result = original(self, operation, payload)
        if operation is Operation.PROPOSE:
            raise InterruptedError("submission reply lost")
        return result

    with monkeypatch.context() as patch:
        patch.setattr(Metadata, "call", interrupt)
        with pytest.raises(InterruptedError):
            Captures(leaves.workspace).capture(identity=leaf.id)
    pending = leaves.read(identity=leaf.id).pending
    assert pending is not None
    assert not pending.submitted
    target.write_bytes(b"later edit")
    with leaves.workspace.session():
        updated = leaves.read(identity=leaf.id).pending
    assert updated is not None
    assert updated.submitted
    assert updated.request == pending.request
    assert updated.changes == pending.changes
    assert target.read_bytes() == b"later edit"
