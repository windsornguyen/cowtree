"""Exercise the Python boundary against the real Rust command interface."""

import hashlib
from pathlib import Path

from pydantic import TypeAdapter
import pytest

from cowtree.errors import CowtreeError
from cowtree.metadata import Metadata, MetadataError
from cowtree.metadata_types import (
    Candidate,
    Entry,
    EntryKind,
    Grant,
    MetadataReason,
    Operation,
    Receipt,
    Request,
    Tip,
)


def test_metadata_roundtrip_publishes_captured_file(tmp_path: Path) -> None:
    binary = Path(__file__).resolve().parents[1] / "target/debug/cowtree-metadata"
    if not binary.is_file():
        pytest.fail("build cowtree-metadata before running the workspace integration tests")
    source = tmp_path / "input"
    source.write_bytes(b"published source")
    with Metadata(root=tmp_path / "authority", binary=binary) as store:
        store.call(Operation.INIT)
        leaf = store.call(Operation.CREATE_LEAF).decode("leaf", TypeAdapter(int))
        grants = store.call(Operation.ACQUIRE, {"leaf": leaf, "paths": ["file"]})
        grant = grants.decode("grants", TypeAdapter(list[Grant]))[0]
        grant = store.call(Operation.ACTIVATE, {"grant": grant.model_dump(mode="json")}).decode(
            "grant", TypeAdapter(Grant)
        )
        digest = store.call(Operation.STAGE_FILE, {"leaf": leaf, "path": str(source)}).decode(
            "object", TypeAdapter(str)
        )
        assert digest == hashlib.sha256(source.read_bytes()).hexdigest()
        entry = Entry(object=digest, kind=EntryKind.FILE)
        store.call(
            Operation.EDIT,
            {"grant": grant.model_dump(mode="json"), "value": entry.model_dump(mode="json")},
        )
        request = Request(leaf=leaf, sequence=1)
        store.call(
            Operation.PROPOSE,
            {"input": {"request": request.model_dump(mode="json"), "paths": ["file"]}},
        )
        candidate = store.call(
            Operation.PREPARE, {"request": request.model_dump(mode="json")}
        ).decode("candidate", TypeAdapter(Candidate))
        receipt = store.call(
            Operation.COMMIT, {"candidate": candidate.model_dump(mode="json")}
        ).decode("receipt", TypeAdapter(Receipt))
        assert receipt.version == 1
        assert store.call(Operation.TIP).decode("tip", TypeAdapter(Tip)).root == receipt.root
        with pytest.raises(MetadataError) as caught:
            store.call(Operation.ACQUIRE, {"leaf": leaf, "paths": ["../escape"]})
        assert caught.value.reason is MetadataReason.INVALID_REQUEST
        assert store.call(Operation.TIP).decode("tip", TypeAdapter(Tip)).version == 1


def test_missing_metadata_executable_is_an_explicit_failure(tmp_path: Path) -> None:
    with (
        pytest.raises(CowtreeError, match="metadata executable unavailable"),
        Metadata(root=tmp_path / "store", binary=tmp_path / "missing"),
    ):
        raise AssertionError("missing binary started")
