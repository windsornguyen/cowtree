"""Process death around grouped flushes cannot publish an incomplete ready record."""

import os
from pathlib import Path
import signal
import subprocess
import sys

import pytest

from .test_fork import manager


@pytest.mark.parametrize("operation", ["seal", "fork"])
@pytest.mark.parametrize("boundary", ["before_full", "after_full", "before_record", "after_record"])
def test_killed_tree_publication_recovers_only_ready_records(
    tmp_path: Path, operation: str, boundary: str
) -> None:
    if sys.platform != "darwin" and boundary.endswith("full"):
        pytest.skip("macOS device-cache barrier")
    leaves = manager(root=tmp_path)
    owner = leaves.fork(path=tmp_path / "owner")
    (owner.path / "file.txt").write_bytes(b"private source")
    target = tmp_path / "child"
    program = """
import os,signal,sys
from pathlib import Path
from cowtree import durable,nodes,leaves
from cowtree.workspace import Workspace
from cowtree.seals import Seals
root,target,identity,operation,boundary=sys.argv[1:]
module=nodes if operation=='seal' else leaves
original_tree=module.sync_tree
original_record=module.write_record
def stop(): os.kill(os.getpid(),signal.SIGKILL)
def tree(root,files,directories):
 original_full=durable.fcntl.fcntl
 def full(descriptor,command):
  if command==51 and boundary=='before_full': stop()
  result=original_full(descriptor,command)
  if command==51 and boundary=='after_full': stop()
  return result
 durable.fcntl.fcntl=full
 try: original_tree(root=root,files=files,directories=directories)
 finally: durable.fcntl.fcntl=original_full
def record(path,record):
 ready=path.name=='node.json' if operation=='seal' else path.parent.name=='leaves'
 if ready and boundary=='before_record': stop()
 original_record(path=path,record=record)
 if ready and boundary=='after_record': stop()
module.sync_tree=tree
module.write_record=record
workspace=Workspace.open(root=Path(root))
if operation=='seal': Seals(workspace).seal(identity=int(identity))
else: leaves.Leaves(workspace).fork(path=Path(target))
"""
    environment = dict(os.environ, PYTHONPATH=str(Path(__file__).resolve().parents[1] / "src"))
    process = subprocess.run(  # noqa: S603
        [
            sys.executable,
            "-c",
            program,
            str(leaves.workspace.root),
            str(target),
            str(owner.id),
            operation,
            boundary,
        ],
        env=environment,
        capture_output=True,
        timeout=60,
        check=False,
    )
    assert process.returncode == -signal.SIGKILL, process.stderr
    leaves.recover()
    assert (owner.path / "file.txt").read_bytes() == b"private source"
    assert (leaves.workspace.config.source / "file.txt").read_bytes() == b"source bytes\n"
    if operation == "fork":
        assert target.exists() == (boundary == "after_record")
        if target.exists():
            assert (target / "file.txt").read_bytes() == b"source bytes\n"
            assert (target / "cache/artifact").read_bytes() == b"warm build"
    else:
        recovered = leaves.read(identity=owner.id)
        assert (recovered.node != owner.node) == (boundary == "after_record")
        if recovered.node != owner.node:
            tree = leaves.workspace.root / "nodes" / recovered.node / "tree"
            assert (tree / "file.txt").read_bytes() == b"private source"
            assert (tree / "cache/artifact").read_bytes() == b"warm build"
    assert list((leaves.workspace.root / "operations").iterdir()) == []


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS device-cache barrier")
@pytest.mark.parametrize("operation", ["seal", "fork"])
def test_failed_device_flush_cannot_create_a_ready_tree_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    from collections.abc import Iterable
    import errno

    from cowtree import durable
    from cowtree.seals import Seals

    leaves = manager(root=tmp_path)
    owner = leaves.fork(path=tmp_path / "owner")
    target = tmp_path / "child"
    before_nodes = sorted((leaves.workspace.root / "nodes").glob("*/node.json"))

    def fail(descriptor: int, command: int) -> int:
        del descriptor, command
        raise OSError(errno.EIO, "injected device-cache failure")

    def group(root: Path, files: Iterable[Path], directories: Iterable[Path]) -> None:
        with monkeypatch.context() as patch:
            patch.setattr(durable.fcntl, "fcntl", fail)
            durable.sync_tree(root=root, files=files, directories=directories)

    module = "cowtree.nodes" if operation == "seal" else "cowtree.leaves"
    with monkeypatch.context() as patch:
        patch.setattr(f"{module}.sync_tree", group)
        if operation == "seal":
            seals = Seals(leaves.workspace)
            with pytest.raises(OSError, match="injected device-cache"):
                seals.seal(identity=owner.id)
        else:
            with pytest.raises(OSError, match="injected device-cache"):
                leaves.fork(path=target)
    assert leaves.records() == [owner]
    assert sorted((leaves.workspace.root / "nodes").glob("*/node.json")) == before_nodes
    leaves.recover()
    assert leaves.records() == [owner]
    assert not target.exists()
    assert (owner.path / "file.txt").read_bytes() == b"source bytes\n"
