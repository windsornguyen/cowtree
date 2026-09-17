"""Acquisition and sync preserve actual working files across authority changes."""

from pathlib import Path

from pydantic import JsonValue, TypeAdapter
import pytest

from cowtree.captures import Captures
from cowtree.durable import write_record
from cowtree.errors import CowtreeError
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.leaves import Leaves
from cowtree.metadata import Metadata, MetadataError
from cowtree.metadata_types import (
    Candidate,
    Entry,
    EntryKind,
    Grant,
    MetadataReason,
    Operation,
    Output,
    Request,
)
from cowtree.views import Views
from cowtree.workspace import Workspace

from .test_fork import manager


def publish_remote(leaves: Leaves, path: str, data: bytes) -> None:
    writer = leaves.fork(path=leaves.workspace.root.parent / "writer")
    target = writer.path / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)
    with leaves.workspace.session() as metadata:
        grant = metadata.call(Operation.ACQUIRE, {"leaf": writer.id, "paths": [path]}).decode(
            "grants", TypeAdapter(list[Grant])
        )[0]
        grant = metadata.call(Operation.ACTIVATE, {"grant": grant.model_dump(mode="json")}).decode(
            "grant", TypeAdapter(Grant)
        )
        digest = metadata.call(
            Operation.STAGE_FILE, {"leaf": writer.id, "path": str(target)}
        ).decode("object", TypeAdapter(str))
        entry = Entry(object=digest, kind=EntryKind.FILE)
        metadata.call(
            Operation.EDIT,
            {"grant": grant.model_dump(mode="json"), "value": entry.model_dump(mode="json")},
        )
        request = Request(leaf=writer.id, sequence=1)
        metadata.call(
            Operation.PROPOSE,
            {"input": {"request": request.model_dump(mode="json"), "paths": [path]}},
        )
        candidate = metadata.call(
            Operation.PREPARE, {"request": request.model_dump(mode="json")}
        ).decode("candidate", TypeAdapter(Candidate))
        metadata.call(Operation.COMMIT, {"candidate": candidate.model_dump(mode="json")})
        refreshed = metadata.call(Operation.ACQUIRE, {"leaf": writer.id, "paths": [path]}).decode(
            "grants", TypeAdapter(list[Grant])
        )[0]
        metadata.call(Operation.RELEASE, {"grant": refreshed.model_dump(mode="json")})
        node = leaves.workspace.nodes.seal(source=writer.path, parent=writer.node)
        config = Workspace.open(root=leaves.workspace.root).config.model_copy(
            update={"warm_tip": node.id}
        )
        write_record(path=leaves.workspace.root / "workspace.json", record=config)


def test_acquisition_preserves_private_edits_and_reserves_new_paths(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "reader")
    (leaf.path / "file.txt").write_bytes(b"private edit")
    views = Views(leaves.workspace)
    activated = views.acquire(identity=leaf.id, paths=("file.txt", "new/path"))
    assert activated.grants["new/path"].activated
    assert (leaf.path / "file.txt").read_bytes() == b"private edit"
    assert not (leaf.path / "new").exists()
    other = leaves.fork(path=tmp_path / "other")
    with pytest.raises(MetadataError) as caught:
        views.acquire(identity=other.id, paths=("file.txt",))
    assert caught.value.reason is MetadataReason.LEASE_CONFLICT


def test_sync_advances_clean_paths_without_replacing_private_edits(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "reader")
    (leaf.path / "file.txt").write_bytes(b"private edit")
    publish_remote(leaves=leaves, path="remote", data=b"published change")
    updated = Views(leaves.workspace).sync(identity=leaf.id)
    assert (leaf.path / "file.txt").read_bytes() == b"private edit"
    assert (leaf.path / "remote").read_bytes() == b"published change"
    assert updated.origins["file.txt"] == leaf.origins["file.txt"]
    assert "remote" in updated.origins


def test_lost_activation_reply_recovers_without_reinstalling(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "reader")
    original = Metadata.call

    def interrupt(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        result = original(self, operation, payload)
        if operation is Operation.ACTIVATE:
            raise InterruptedError("activation reply lost")
        return result

    views = Views(leaves.workspace)
    with monkeypatch.context() as patch:
        patch.setattr(Metadata, "call", interrupt)
        with pytest.raises(InterruptedError):
            views.acquire(identity=leaf.id, paths=("file.txt",))
    with leaves.workspace.session() as metadata:
        views.recover_locked(metadata=metadata)
    assert leaves.read(identity=leaf.id).grants["file.txt"].activated
    assert (leaf.path / "file.txt").read_bytes() == b"source bytes\n"
    assert list((leaves.workspace.root / "operations").iterdir()) == []


def test_sync_does_not_move_a_callers_branch(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "reader")
    repository = GitRepository(io=CommandRunner(), path=leaf.path)
    repository.run(args=["checkout", "-b", "caller-owned"])
    with pytest.raises(CowtreeError, match="must remain detached"):
        Views(leaves.workspace).sync(identity=leaf.id)


def test_sync_preserves_a_post_capture_revert_to_the_old_origin(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "reader")
    target = leaf.path / "file.txt"
    target.write_bytes(b"submitted")
    pending = Captures(leaves.workspace).capture(identity=leaf.id)
    assert pending is not None
    with leaves.workspace.session() as metadata:
        metadata.call(Operation.REVOKE, {"path": "file.txt"})
    target.write_bytes(b"source bytes\n")
    publish_remote(leaves=leaves, path="file.txt", data=b"another publication")
    updated = Views(leaves.workspace).sync(identity=leaf.id)
    assert target.read_bytes() == b"source bytes\n"
    assert updated.origins["file.txt"] == leaf.origins["file.txt"]
    assert updated.pending == pending


def test_stale_activation_rolls_back_installed_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "reader")
    publish_remote(leaves=leaves, path="file.txt", data=b"new origin")
    original = Metadata.call

    def revoke(
        self: Metadata, operation: Operation, payload: dict[str, JsonValue] | None = None
    ) -> Output:
        if operation is Operation.ACTIVATE:
            original(self, Operation.REVOKE, {"path": "file.txt"})
        return original(self, operation, payload)

    views = Views(leaves.workspace)
    with monkeypatch.context() as patch:
        patch.setattr(Metadata, "call", revoke)
        with pytest.raises(MetadataError) as caught:
            views.acquire(identity=leaf.id, paths=("file.txt",))
    assert caught.value.reason is MetadataReason.STALE_TOKEN
    assert (leaf.path / "file.txt").read_bytes() == b"new origin"
    with leaves.workspace.session() as metadata:
        views.recover_locked(metadata=metadata)
    assert (leaf.path / "file.txt").read_bytes() == b"source bytes\n"
    assert list((leaves.workspace.root / "operations").iterdir()) == []
