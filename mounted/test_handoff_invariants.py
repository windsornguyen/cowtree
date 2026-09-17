"""Retained workspace state remains usable across ordinary maintenance and recovery."""

from pathlib import Path
import sys

import pytest

from cowtree.captures import Captures
from cowtree.checks import Checks
from cowtree.collection import Collector
from cowtree.publications import Publications
from cowtree.views import Views

from .test_fork import manager


def test_collection_keeps_an_unleased_leaf_origin_discardable(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    reader = leaves.fork(path=tmp_path / "reader")
    writer = leaves.fork(path=tmp_path / "writer")
    publications = Publications(leaves.workspace)
    for version in range(17):
        (writer.path / "file.txt").write_text(f"published {version}")
        Captures(leaves.workspace).capture(identity=writer.id)
        candidate = publications.prepare(identity=writer.id)
        Checks(leaves.workspace).validate(
            identity=writer.id, command=(sys.executable, "-c", "pass")
        )
        publications.commit(candidate=candidate)
    Collector(leaves.workspace).collect()
    (reader.path / "file.txt").write_bytes(b"private edit")
    Views(leaves.workspace).discard(identity=reader.id, paths=("file.txt",))
    assert (reader.path / "file.txt").read_bytes() == b"source bytes\n"


def test_initialization_recovery_handles_interruption_before_its_first_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from cowtree.tree_types import PathPolicy
    from cowtree.workspace import Initialization, Workspace

    from .test_bootstrap import metadata_binary, source_repository

    source = source_repository(path=tmp_path / "source")
    root = tmp_path / "workspace"

    def interrupt(path: Path, record: Initialization) -> None:
        del path, record
        raise InterruptedError("before first initialization record")

    with monkeypatch.context() as patch:
        patch.setattr("cowtree.workspace.write_record", interrupt)
        with pytest.raises(InterruptedError):
            Workspace.create(
                root=root, source=source, binary=metadata_binary(), policy=PathPolicy()
            )
    assert Workspace.recover_initialization(root=root) is False
    assert not Workspace.staging(root=root).exists()


def test_recovery_waits_for_live_initialization_and_observes_its_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from concurrent.futures import ThreadPoolExecutor
    import fcntl
    from threading import Event, current_thread

    from cowtree.publication import publish_directory
    from cowtree.tree_types import PathPolicy
    from cowtree.workspace import Workspace

    from .test_bootstrap import metadata_binary, source_repository

    source = source_repository(path=tmp_path / "source")
    root = tmp_path / "workspace"
    publication_ready, recovery_waiting, release_creator = Event(), Event(), Event()
    original_flock = fcntl.flock

    def pause(source: Path, target: Path) -> None:
        if target == root:
            publication_ready.set()
            assert release_creator.wait(timeout=10)
        publish_directory(source=source, target=target)

    def observe_lock(descriptor: int, operation: int) -> None:
        if current_thread().name.startswith("recover") and operation == fcntl.LOCK_EX:
            recovery_waiting.set()
        original_flock(descriptor, operation)

    monkeypatch.setattr("cowtree.workspace.publish_directory", pause)
    monkeypatch.setattr("cowtree.workspace.fcntl.flock", observe_lock)
    with ThreadPoolExecutor(max_workers=1, thread_name_prefix="creator") as creator:
        create = creator.submit(Workspace.create, root, source, metadata_binary(), PathPolicy())
        assert publication_ready.wait(timeout=10)
        with ThreadPoolExecutor(max_workers=1, thread_name_prefix="recover") as recovery:
            recover = recovery.submit(Workspace.recover_initialization, root)
            try:
                assert recovery_waiting.wait(timeout=10)
            finally:
                release_creator.set()
            assert create.result(timeout=10).root == root.resolve()
            assert recover.result(timeout=10)


def test_late_initializer_does_not_leave_staging_behind_a_ready_workspace(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from concurrent.futures import ThreadPoolExecutor
    from threading import Event, current_thread

    from cowtree.errors import CowtreeError
    from cowtree.publication import publish_directory
    from cowtree.tree_types import PathPolicy
    from cowtree.workspace import Workspace

    from .test_bootstrap import metadata_binary, source_repository

    source = source_repository(path=tmp_path / "source")
    root = tmp_path / "workspace"
    waiting, proceed = Event(), Event()

    def pause(source: Path, target: Path) -> None:
        if target == Workspace.staging(root=root) and current_thread().name.startswith("late"):
            waiting.set()
            assert proceed.wait(timeout=10)
        publish_directory(source=source, target=target)

    monkeypatch.setattr("cowtree.workspace.publish_directory", pause)
    with ThreadPoolExecutor(max_workers=1, thread_name_prefix="late") as executor:
        late = executor.submit(Workspace.create, root, source, metadata_binary(), PathPolicy())
        assert waiting.wait(timeout=10)
        try:
            ready = Workspace.create(
                root=root, source=source, binary=metadata_binary(), policy=PathPolicy()
            )
        finally:
            proceed.set()
        with pytest.raises(CowtreeError):
            late.result(timeout=10)
    assert Workspace.open(root=root).config == ready.config
    assert not Workspace.staging(root=root).exists()
