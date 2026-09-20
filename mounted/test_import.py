"""Initial import is bounded, observable, and resumes the same frozen source."""

import os
from pathlib import Path
import signal
import subprocess
import sys

from pydantic import JsonValue
import pytest

from cowtree.metadata import Metadata
from cowtree.metadata_types import Operation, Output
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary, source_repository


def test_initial_import_does_not_create_per_file_views(tmp_path: Path) -> None:
    import sqlite3

    source = source_repository(path=tmp_path / "source")
    for index in range(130):
        (source / f"file-{index}").write_text(f"{index}\n")
    workspace = Workspace.create(
        root=tmp_path / "store", source=source, binary=metadata_binary(), policy=PathPolicy()
    )
    with sqlite3.connect(workspace.root / "authority/metadata.sqlite3") as connection:
        assert connection.execute("select next_leaf from settings").fetchone()[0] == 1
    assert len(workspace.nodes.read(identity=workspace.config.initial).source) == 132


def test_import_resumes_the_frozen_snapshot_after_a_lost_chunk_reply(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = source_repository(path=tmp_path / "source")
    for index in range(130):
        (source / f"file-{index}").write_text(f"{index}\n")
    root = tmp_path / "store"
    original = Metadata.call

    def interrupt(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        output = original(self, operation, payload)
        if operation.value == "import_chunk":
            raise InterruptedError("chunk reply lost")
        return output

    with monkeypatch.context() as patch:
        patch.setattr(Metadata, "call", interrupt)
        with pytest.raises(InterruptedError):
            Workspace.create(
                root=root, source=source, binary=metadata_binary(), policy=PathPolicy()
            )
    status = Workspace.import_status(root=root)
    assert status is not None
    assert status.completed == 64
    assert not status.complete
    (source / "file-0").write_text("later source edit")
    assert Workspace.recover_initialization(root=root)
    workspace = Workspace.open(root=root)
    node = workspace.nodes.read(identity=workspace.config.initial)
    assert (root / "nodes" / node.id / "tree/file-0").read_text() == "0\n"
    assert (source / "file-0").read_text() == "later source edit"
    progress = Workspace.import_status(root=root)
    assert progress is not None
    assert progress.complete


@pytest.mark.parametrize("boundary", ["begin_import", "import_chunk", "finish_import"])
def test_process_kill_resumes_without_changing_captured_bytes(
    tmp_path: Path, boundary: str
) -> None:
    source = source_repository(path=tmp_path / "source")
    for index in range(70):
        (source / f"source-{index}").write_text(str(index))
    root = tmp_path / "store"
    program = (
        "import os,signal,sys\nfrom pathlib import Path\n"
        "from cowtree.metadata import Metadata\nfrom cowtree.workspace import Workspace\n"
        "from cowtree.tree_types import PathPolicy\n"
        "original=Metadata.call\n"
        "def kill(self,operation,payload=None):\n"
        " result=original(self,operation,payload)\n"
        " if operation.value==sys.argv[4]: os.kill(os.getpid(),signal.SIGKILL)\n"
        " return result\n"
        "Metadata.call=kill\n"
        "Workspace.create(root=Path(sys.argv[1]),source=Path(sys.argv[2]),"
        "binary=Path(sys.argv[3]),policy=PathPolicy())\n"
    )
    environment = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1] / "src"))
    process = subprocess.run(  # noqa: S603
        [sys.executable, "-c", program, str(root), str(source), str(metadata_binary()), boundary],
        env=environment,
        capture_output=True,
        timeout=60,
        check=False,
    )
    assert process.returncode == -signal.SIGKILL, process.stderr
    assert Workspace.recover_initialization(root=root)
    workspace = Workspace.open(root=root)
    node = workspace.nodes.read(identity=workspace.config.initial)
    assert len(node.source) == 72
    for index in range(70):
        entry = node.source[f"source-{index}"]
        assert (root / "authority/objects" / entry.object).read_text() == str(index)
        assert (source / f"source-{index}").read_text() == str(index)
