"""Explicit resolution rebases private edits without publishing them implicitly."""

from pathlib import Path
import sys

import pytest

from cowtree.captures import Captures
from cowtree.checks import Checks
from cowtree.errors import CowtreeError
from cowtree.metadata_types import Operation
from cowtree.publications import Publications
from cowtree.resolutions import Choice, Resolutions

from .test_fork import manager


@pytest.mark.parametrize("choice", [Choice.LOCAL, Choice.PUBLISHED])
def test_resolution_requires_a_choice_for_stale_private_content(
    tmp_path: Path, choice: Choice
) -> None:
    leaves = manager(root=tmp_path)
    first = leaves.fork(path=tmp_path / "first")
    second = leaves.fork(path=tmp_path / "second")
    (second.path / "file.txt").write_bytes(b"private")
    (first.path / "file.txt").write_bytes(b"published")
    Captures(leaves.workspace).capture(identity=first.id)
    publications = Publications(leaves.workspace)
    candidate = publications.prepare(identity=first.id)
    Checks(leaves.workspace).validate(identity=first.id, command=(sys.executable, "-c", "pass"))
    publications.commit(candidate=candidate)
    with leaves.workspace.session() as metadata:
        metadata.call(Operation.REVOKE, {"path": "file.txt"})
    with pytest.raises(CowtreeError, match="explicit resolution"):
        Captures(leaves.workspace).capture(identity=second.id)
    Resolutions(leaves.workspace).resolve(identity=second.id, choices={"file.txt": choice})
    expected = b"private" if choice is Choice.LOCAL else b"published"
    assert (second.path / "file.txt").read_bytes() == expected
    next_capture = Captures(leaves.workspace).capture(identity=second.id)
    assert (next_capture is None) == (choice is Choice.PUBLISHED)
