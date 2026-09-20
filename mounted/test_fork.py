"""Warm forks are real locked Git worktrees with recoverable ownership."""

from concurrent.futures import ThreadPoolExecutor
import fcntl
import os
from pathlib import Path
import subprocess
import sys
from threading import Barrier

from pydantic import TypeAdapter
import pytest

from cowtree.captures import Captures
from cowtree.durable import write_record
from cowtree.errors import CowtreeError
from cowtree.leaves import ForkRecord, Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.metadata_types import Candidate, Operation, Request
from cowtree.tree_types import CaptureMode, PathPolicy, TreeEntry
from cowtree.trees import populate_tree
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary, source_repository


def manager(root: Path) -> Leaves:
    source = source_repository(path=root / "source")
    workspace = Workspace.create(
        root=root / "workspace",
        source=source,
        binary=metadata_binary(),
        policy=PathPolicy(derived=("cache",)),
    )
    result = Leaves(workspace)
    return result


def test_warm_fork_is_registered_and_has_private_cache_bytes(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "leaf")
    assert (leaf.path / "cache/artifact").read_bytes() == b"warm build"
    assert not (leaf.path / ".env").exists()
    tree = leaves.workspace.repository.find_worktree(path=leaf.path)
    assert tree is not None
    assert tree.locked
    (leaf.path / "cache/artifact").write_bytes(b"new cache")
    assert (leaves.workspace.config.source / "cache/artifact").read_bytes() == b"warm build"
    assert leaves.read(identity=leaf.id) == leaf
    assert list((leaves.workspace.root / "operations").iterdir()) == []


def test_validation_leaves_cannot_publish_or_disappear_under_a_live_check(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    owner = leaves.fork(path=tmp_path / "owner")
    candidate = Candidate(
        request=Request(leaf=owner.id, sequence=1), attempt=1, parent=0, root="0" * 64
    )
    check = leaves.fork(path=tmp_path / "check", check_candidate=candidate)
    with pytest.raises(CowtreeError, match="check views cannot publish"):
        Captures(leaves.workspace).capture(identity=check.id)
    path = leaves.workspace.root / "checks" / f"{owner.id}-1.lock"
    with path.open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        with pytest.raises(CowtreeError, match="validation is still running"):
            Lifecycle(leaves.workspace).drop(identity=check.id, force=True)
        assert check.path.exists()
        with leaves.workspace.session():
            assert check.path.exists()
    Lifecycle(leaves.workspace).drop(identity=check.id, force=True)
    assert not check.path.exists()


def test_independent_forks_reach_the_clone_phase_concurrently(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    barrier = Barrier(2, timeout=10)

    def overlap(
        source: Path, target: Path, policy: PathPolicy, *, capture: CaptureMode
    ) -> tuple[TreeEntry, ...]:
        barrier.wait()
        return populate_tree(source=source, target=target, policy=policy, capture=capture)

    monkeypatch.setattr("cowtree.leaves.populate_tree", overlap)
    with ThreadPoolExecutor(max_workers=2) as workers:
        first = workers.submit(leaves.fork, path=tmp_path / "first")
        second = workers.submit(leaves.fork, path=tmp_path / "second")
        left, right = first.result(timeout=30), second.result(timeout=30)
    assert left.id != right.id
    assert {leaf.id for leaf in leaves.records()} == {left.id, right.id}


def test_failed_population_recovery_removes_only_the_owned_fork(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    retained = leaves.fork(path=tmp_path / "retained")

    def interrupt(
        source: Path, target: Path, policy: PathPolicy, *, capture: CaptureMode
    ) -> tuple[TreeEntry, ...]:
        populate_tree(source=source, target=target, policy=policy, capture=capture)
        raise InterruptedError("after population")

    with monkeypatch.context() as patch:
        patch.setattr("cowtree.leaves.populate_tree", interrupt)
        with pytest.raises(InterruptedError):
            leaves.fork(path=tmp_path / "failed")
    leaves.recover()
    assert not (tmp_path / "failed").exists()
    assert retained.path.is_dir()
    with leaves.workspace.session() as metadata:
        assert metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int])) == [
            retained.id
        ]


def test_uncertain_leaf_allocation_is_retired_before_another_fork(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)

    def interrupt(path: Path, record: ForkRecord) -> None:
        if record.leaf is not None:
            raise InterruptedError("after allocation")
        write_record(path=path, record=record)

    with monkeypatch.context() as patch:
        patch.setattr("cowtree.leaves.write_record", interrupt)
        with pytest.raises(InterruptedError):
            leaves.fork(path=tmp_path / "uncertain")
    ready = leaves.fork(path=tmp_path / "ready")
    with leaves.workspace.session() as metadata:
        assert metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int])) == [
            ready.id
        ]
    assert not (tmp_path / "uncertain").exists()


@pytest.mark.parametrize("phase", ["copy", "ready"])
def test_process_exit_preserves_exactly_the_ready_fork(tmp_path: Path, phase: str) -> None:
    leaves = manager(root=tmp_path)
    target = tmp_path / "child"
    program = (
        "import os,sys\nfrom pathlib import Path\n"
        "import cowtree.leaves as module\nfrom cowtree.workspace import Workspace\n"
        "if sys.argv[3]=='copy':\n"
        " original=module.populate_tree\n"
        " def crash(source,target,policy,*,capture):\n"
        "  original(source=source,target=target,policy=policy,capture=capture); os._exit(99)\n"
        " module.populate_tree=crash\n"
        "else:\n"
        " original=module.Leaves.finish\n"
        " def crash(self,metadata,record,node,descriptor):\n"
        "  original(self,metadata,record,node,descriptor); os._exit(99)\n"
        " module.Leaves.finish=crash\n"
        "module.Leaves(Workspace.open(Path(sys.argv[1]))).fork(Path(sys.argv[2]))\n"
    )
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1] / "src")
    result = subprocess.run(  # noqa: S603
        [sys.executable, "-c", program, str(leaves.workspace.root), str(target), phase],
        env=environment,
        capture_output=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 99, result.stderr.decode()
    recovered = leaves.recover()
    with leaves.workspace.session() as metadata:
        active = metadata.call(Operation.LEAVES).decode("leaves", TypeAdapter(list[int]))
    if phase == "ready":
        assert active == recovered
        assert len(active) == 1
        assert (target / "file.txt").read_bytes() == b"source bytes\n"
    else:
        assert active == []
        assert not target.exists()
    assert list((leaves.workspace.root / "operations").iterdir()) == []
