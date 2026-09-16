"""The process boundary preserves wire errors and rejects malformed results."""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import subprocess

import pytest

from cowtree.metadata_client import MetadataClient
from cowtree.workspace_types import Json, MetadataError, RetryAction, WireRecord


@dataclass
class Reply:
    value: Json

    def run(
        self,
        args: list[str],
        input: bytes,
        capture_output: bool,
        check: bool,
    ) -> subprocess.CompletedProcess[bytes]:
        assert json.loads(input) == {"op": "tip"}
        assert capture_output
        assert not check
        result = subprocess.CompletedProcess(
            args=args,
            returncode=0,
            stdout=json.dumps(self.value).encode(),
            stderr=b"",
        )
        return result


def test_missing_native_authority_fails_explicitly(tmp_path: Path) -> None:
    client = MetadataClient(authority=tmp_path / "authority", binary=tmp_path / "missing")
    with pytest.raises(MetadataError) as error:
        client.call({"op": "tip"}, "tip")
    assert error.value.code == "metadata_unavailable"


def test_wire_failure_preserves_retry_and_structured_context(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    reply = Reply(
        {
            "status": "error",
            "message": "a diagnostic may change",
            "code": "tip_changed",
            "retry_action": "reprepare",
            "details": {"kind": "tip", "expected": 1, "actual": 2},
        }
    )
    monkeypatch.setattr(subprocess, "run", reply.run)
    client = MetadataClient(authority=tmp_path, binary=Path("/unused"))
    with pytest.raises(MetadataError) as error:
        client.call({"op": "tip"}, "tip")
    assert error.value.code == "tip_changed"
    assert error.value.retry_action is RetryAction.REPREPARE
    assert error.value.details is not None
    assert error.value.details.number("actual") == 2


@pytest.mark.parametrize(
    "value",
    [
        {"status": "unexpected"},
        {"status": "ok", "output": {"kind": "leaf", "value": 1}},
        {"status": "ok", "output": {"kind": "tip"}},
        {"status": "error", "code": "tip_changed", "message": "missing retry context"},
        [],
    ],
)
def test_malformed_authority_response_fails_closed(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    value: Json,
) -> None:
    monkeypatch.setattr(subprocess, "run", Reply(value).run)
    client = MetadataClient(authority=tmp_path, binary=Path("/unused"))
    with pytest.raises(MetadataError) as error:
        client.call({"op": "tip"}, "tip")
    assert error.value.code == "invalid_response"


@pytest.mark.parametrize("value", [True, -1, 0.5, "1"])
def test_wire_identifiers_are_integers(value: Json) -> None:
    with pytest.raises(MetadataError) as error:
        WireRecord({"leaf": value}).number("leaf")
    assert error.value.code == "invalid_response"
