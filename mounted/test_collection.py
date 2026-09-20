"""Collection retains explicit and live roots and removes released private images."""

import os
from pathlib import Path
import subprocess
import sys

import pytest

from cowtree.collection import Collector
from cowtree.leaves import Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.seals import Seals
from cowtree.workspace import Workspace

from .test_fork import manager


def test_collection_preserves_live_descendants_and_manual_retention(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    (leaf.path / "file.txt").write_bytes(b"first private")
    seals = Seals(leaves.workspace)
    first = seals.seal(identity=leaf.id)
    child = leaves.fork(path=tmp_path / "child", node=first.id)
    (leaf.path / "file.txt").write_bytes(b"second private")
    second = seals.seal(identity=leaf.id)
    seals.release(identity=first.id)
    report = Collector(leaves.workspace).collect()
    assert first.id not in report.nodes
    assert (child.path / "file.txt").read_bytes() == b"first private"
    Lifecycle(leaves.workspace).drop(identity=child.id, force=True)
    report = Collector(leaves.workspace).collect()
    assert first.id in report.nodes
    assert not (leaves.workspace.root / "nodes" / first.id).exists()
    assert (leaves.workspace.root / "nodes" / second.id).exists()


def test_active_validation_keeps_its_view_and_log(tmp_path: Path) -> None:
    import fcntl

    from cowtree.metadata_types import Candidate, Request

    leaves = manager(root=tmp_path)
    owner = leaves.fork(path=tmp_path / "owner")
    candidate = Candidate(
        request=Request(leaf=owner.id, sequence=1), attempt=1, parent=0, root="0" * 64
    )
    check = leaves.fork(path=tmp_path / "check", check_candidate=candidate)
    checks = leaves.workspace.root / "checks"
    log = checks / f"{check.id}.log"
    log.write_bytes(b"running check")
    os.utime(log, ns=(1, 1))
    for identity in range(1000, 1065):
        (checks / f"{identity}.log").write_bytes(b"finished")
    with (checks / f"{owner.id}-1.lock").open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        report = Collector(leaves.workspace).collect()
        assert check.id not in report.checks
        assert check.path.is_dir()
        assert log.read_bytes() == b"running check"
    report = Collector(leaves.workspace).collect()
    assert check.id in report.checks
    assert not check.path.exists()


def test_unacknowledged_partial_capture_is_collectible(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    program = (
        "import os,sys\nfrom pathlib import Path\n"
        "import cowtree.nodes as module\nfrom cowtree.workspace import Workspace\n"
        "original=module.clone_tree\n"
        "def crash(source,target,policy):\n"
        " original(source=source,target=target,policy=policy); os._exit(99)\n"
        "module.clone_tree=crash\n"
        "workspace=Workspace.open(Path(sys.argv[1]))\n"
        "with workspace.session():\n"
        " workspace.nodes.seal(source=workspace.config.source)\n"
    )
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1] / "src")
    result = subprocess.run(  # noqa: S603
        [sys.executable, "-c", program, str(leaves.workspace.root)],
        env=environment,
        capture_output=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 99, result.stderr.decode()
    partials = [
        path
        for path in (leaves.workspace.root / "nodes").iterdir()
        if path.name != leaves.workspace.config.initial
    ]
    assert len(partials) == 1
    partial = partials[0]
    assert not (partial / "node.json").exists()
    report = Collector(leaves.workspace).collect()
    assert partial.name in report.nodes
    assert not partial.exists()
    assert (leaves.workspace.root / "nodes" / leaves.workspace.config.initial).is_dir()


@pytest.mark.parametrize("phase", ["marker", "rename", "ref", "delete"])
def test_process_restart_finishes_quarantine(tmp_path: Path, phase: str) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    (leaf.path / "file.txt").write_bytes(b"private checkpoint")
    obsolete = Seals(leaves.workspace).seal(identity=leaf.id, retain=False)
    Lifecycle(leaves.workspace).drop(identity=leaf.id, force=True)
    program = (
        "import os,sys\nfrom pathlib import Path\n"
        "import cowtree.collection as module\nfrom cowtree.workspace import Workspace\n"
        "phase=sys.argv[2]\n"
        "if phase=='marker':\n"
        " original=module.write_record\n"
        " def crash(path,record):\n"
        "  original(path=path,record=record); os._exit(99)\n"
        " module.write_record=crash\n"
        "elif phase=='rename':\n"
        " original=module.publish_directory\n"
        " def crash(source,target):\n"
        "  original(source=source,target=target); os._exit(99)\n"
        " module.publish_directory=crash\n"
        "elif phase=='ref':\n"
        " original=module.GitProjection.remove_ref\n"
        " def crash(self,reference,expected):\n"
        "  original(self,reference=reference,expected=expected); os._exit(99)\n"
        " module.GitProjection.remove_ref=crash\n"
        "else:\n"
        " def crash(path):\n"
        "  (path/'node.json').unlink(); os._exit(99)\n"
        " module.shutil.rmtree=crash\n"
        "module.Collector(Workspace.open(Path(sys.argv[1]))).collect()\n"
    )
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1] / "src")
    result = subprocess.run(  # noqa: S603
        [sys.executable, "-c", program, str(leaves.workspace.root), phase],
        env=environment,
        capture_output=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 99, result.stderr.decode()
    reopened = Leaves(Workspace.open(root=leaves.workspace.root))
    reopened.recover()
    assert not (leaves.workspace.root / "nodes" / obsolete.id).exists()
    assert list((leaves.workspace.root / "trash").iterdir()) == []
    refs = leaves.workspace.repository.capture(args=["for-each-ref", "refs/cowtree/nodes"])
    assert obsolete.id not in refs
    child = reopened.fork(path=tmp_path / "after-restart")
    assert (child.path / "file.txt").read_bytes() == b"source bytes\n"


def test_inflight_fork_pins_a_released_checkpoint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from concurrent.futures import ThreadPoolExecutor
    from threading import Event

    from cowtree.tree_types import CaptureMode, PathPolicy, TreeEntry
    from cowtree.trees import populate_tree

    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    (leaf.path / "file.txt").write_bytes(b"clone input")
    node = Seals(leaves.workspace).seal(identity=leaf.id, retain=False)
    entered, resume = Event(), Event()

    def paused(
        source: Path, target: Path, policy: PathPolicy, *, capture: CaptureMode
    ) -> tuple[TreeEntry, ...]:
        entered.set()
        assert resume.wait(timeout=15)
        result = populate_tree(source=source, target=target, policy=policy, capture=capture)
        return result

    with monkeypatch.context() as patch, ThreadPoolExecutor(max_workers=1) as workers:
        patch.setattr("cowtree.leaves.populate_tree", paused)
        fork = workers.submit(leaves.fork, path=tmp_path / "child", node=node.id)
        try:
            assert entered.wait(timeout=15)
            Lifecycle(leaves.workspace).drop(identity=leaf.id, force=True)
            report = Collector(leaves.workspace).collect()
            assert node.id not in report.nodes
            assert (
                leaves.workspace.root / "nodes" / node.id / "tree/file.txt"
            ).read_bytes() == b"clone input"
        finally:
            resume.set()
        child = fork.result(timeout=15)
    assert (child.path / "file.txt").read_bytes() == b"clone input"
    Lifecycle(leaves.workspace).drop(identity=child.id, force=True)
    assert node.id in Collector(leaves.workspace).collect().nodes


def test_receipts_and_pending_validation_keep_their_logs(tmp_path: Path) -> None:
    from cowtree.captures import Captures
    from cowtree.checks import Checks
    from cowtree.durable import write_record
    from cowtree.metadata_types import Candidate, Receipt, Request
    from cowtree.publications import Publications
    from cowtree.workspace_types import PublicationRecord, Validation

    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    (leaf.path / "file.txt").write_bytes(b"pending publication")
    Captures(leaves.workspace).capture(identity=leaf.id)
    Publications(leaves.workspace).prepare(identity=leaf.id)
    validation = Checks(leaves.workspace).validate(
        identity=leaf.id, command=(sys.executable, "-c", "pass")
    )
    os.utime(validation.log, ns=(1, 1))
    checks = leaves.workspace.root / "checks"
    for version in range(1, 130):
        log = checks / f"{1000 + version}.log"
        log.write_text(f"validation {version}")
        os.utime(log, ns=(version + 1, version + 1))
        request = Request(leaf=1000, sequence=version)
        candidate = Candidate(request=request, attempt=1, parent=version - 1, root="0" * 64)
        record = PublicationRecord(
            receipt=Receipt(request=request, version=version, root=candidate.root),
            validation=Validation(
                candidate=candidate, command=("true",), log=str(log), node=validation.node
            ),
        )
        write_record(path=leaves.workspace.root / "receipts" / f"{version}.json", record=record)
    report = Collector(leaves.workspace).collect()
    assert report.metadata.wal_bytes == 0
    assert validation.node not in report.nodes
    assert Path(validation.log).is_file()
    assert not (leaves.workspace.root / "receipts/1.json").exists()
    assert not (checks / "1001.log").exists()
    assert len(list((leaves.workspace.root / "receipts").glob("*.json"))) == 128
    assert all((checks / f"{1000 + version}.log").is_file() for version in range(2, 130))
    assert Collector(leaves.workspace).collect().nodes == []


def test_unknown_operation_preserves_unreferenced_images(tmp_path: Path) -> None:
    from cowtree.errors import CowtreeError

    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    node = Seals(leaves.workspace).seal(identity=leaf.id, retain=False)
    Lifecycle(leaves.workspace).drop(identity=leaf.id, force=True)
    (leaves.workspace.root / "operations" / "unknown").mkdir()
    with pytest.raises(CowtreeError, match="unrecognized operation"):
        Collector(leaves.workspace).collect()
    assert (leaves.workspace.root / "nodes" / node.id / "node.json").is_file()
