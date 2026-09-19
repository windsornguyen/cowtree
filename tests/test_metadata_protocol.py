"""Protocol errors remain failures and cannot leave a child process running."""

from pathlib import Path
import sys

from pydantic import TypeAdapter, ValidationError
import pytest

from cowtree.errors import CowtreeError
from cowtree.metadata import Metadata
from cowtree.metadata_types import Operation, Output


def test_partial_response_does_not_extend_the_deadline(tmp_path: Path) -> None:
    binary = tmp_path / "partial-response"
    binary.write_text(
        f"#!{sys.executable}\nimport sys, time\n"
        "sys.stdin.readline()\nsys.stdout.write('{')\nsys.stdout.flush()\ntime.sleep(30)\n"
    )
    binary.chmod(0o700)
    session = Metadata(root=tmp_path, binary=binary, timeout_seconds=0.1)
    with pytest.raises(CowtreeError, match="response timed out"), session:
        session.call(Operation.TIP)
    assert session.process is None


def test_boolean_response_cannot_become_a_leaf_identity() -> None:
    output = Output(kind="leaf", value=True)
    with pytest.raises(ValidationError):
        output.decode("leaf", TypeAdapter(int))


def test_response_tag_must_match_the_requested_operation() -> None:
    output = Output(kind="object", value=7)
    with pytest.raises(ValueError, match="expected metadata output leaf"):
        output.decode("leaf", TypeAdapter(int))
