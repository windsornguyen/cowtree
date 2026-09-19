"""The checked candidate is published once while later working edits survive."""

from pathlib import Path
import sys

from pydantic import JsonValue
import pytest

from cowtree.captures import Captures
from cowtree.checks import Checks
from cowtree.errors import CowtreeError
from cowtree.metadata import Metadata
from cowtree.metadata_types import Operation, Output
from cowtree.publications import Publications

from .test_fork import manager


def test_checked_publication_preserves_later_edits_and_warms_new_forks(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "writer")
    (leaf.path / "file.txt").write_bytes(b"captured")
    Captures(leaves.workspace).capture(identity=leaf.id)
    publications = Publications(leaves.workspace)
    candidate = publications.prepare(identity=leaf.id)
    with pytest.raises(CowtreeError, match="not passed validation"):
        publications.commit(candidate=candidate)
    command = (
        sys.executable,
        "-c",
        "from pathlib import Path; assert Path('file.txt').read_bytes()==b'captured'; "
        "Path('cache/artifact').write_bytes(b'checked build')",
    )
    validation = Checks(leaves.workspace).validate(identity=leaf.id, command=command)
    assert validation.candidate == candidate
    (leaf.path / "file.txt").write_bytes(b"later edit")
    receipt = publications.commit(candidate=candidate)
    assert publications.commit(candidate=candidate) == receipt
    assert publications.result(request=receipt.request) == receipt
    assert (leaf.path / "file.txt").read_bytes() == b"later edit"
    assert leaves.read(identity=leaf.id).pending is None
    reader = leaves.fork(path=tmp_path / "reader")
    assert (reader.path / "file.txt").read_bytes() == b"captured"
    assert (reader.path / "cache/artifact").read_bytes() == b"checked build"


def test_validation_cannot_modify_the_candidate_source(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "writer")
    (leaf.path / "file.txt").write_bytes(b"captured")
    Captures(leaves.workspace).capture(identity=leaf.id)
    candidate = Publications(leaves.workspace).prepare(identity=leaf.id)
    command = (
        sys.executable,
        "-c",
        "from pathlib import Path; Path('file.txt').write_bytes(b'changed by checks')",
    )
    with pytest.raises(CowtreeError, match="changed source"):
        Checks(leaves.workspace).validate(identity=leaf.id, command=command)
    with pytest.raises(CowtreeError, match="not passed validation"):
        Publications(leaves.workspace).commit(candidate=candidate)


def test_lost_commit_reply_recovers_the_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "writer")
    (leaf.path / "file.txt").write_bytes(b"captured")
    Captures(leaves.workspace).capture(identity=leaf.id)
    publications = Publications(leaves.workspace)
    candidate = publications.prepare(identity=leaf.id)
    Checks(leaves.workspace).validate(identity=leaf.id, command=(sys.executable, "-c", "pass"))
    original = Metadata.call

    def interrupt(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        output = original(self, operation, payload)
        if operation is Operation.COMMIT:
            raise InterruptedError("commit acknowledgement lost")
        return output

    with monkeypatch.context() as patch:
        patch.setattr(Metadata, "call", interrupt)
        with pytest.raises(InterruptedError):
            publications.commit(candidate=candidate)
    assert leaves.read(identity=leaf.id).pending is not None
    receipt = publications.result(request=candidate.request)
    assert receipt is not None
    assert leaves.read(identity=leaf.id).last_receipt == receipt
    assert publications.commit(candidate=candidate) == receipt
