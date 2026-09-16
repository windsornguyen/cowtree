"""Managed worktree publication uses real Git trees and the Rust authority."""

from __future__ import annotations

import concurrent.futures
from dataclasses import dataclass
import json
import os
from pathlib import Path
import subprocess
import threading
import time

import pytest

from cowtree.metadata_client import MetadataClient
from cowtree.workspace import Workspace, finish_import, workspace_lock
from cowtree.workspace_types import (
    Json,
    MetadataError,
    RequestId,
    RetryAction,
    WireRecord,
    WorkspaceLeaf,
)
from tests.conftest import Repository


def test_publication_updates_real_worktree_and_preserves_git(
    workspace: Workspace,
    tmp_path: Path,
) -> None:
    writer = workspace.add(tmp_path / "writer")
    reader = workspace.add(tmp_path / "reader")
    grant = workspace.activate(workspace.acquire(writer, ["file.txt"])[0])
    (writer.path / "file.txt").write_bytes(b"published\n")
    views = workspace.capture([grant])
    assert views[0].dirty
    receipt = workspace.publish(RequestId(writer.leaf, 1), ["file.txt"])
    workspace.sync(reader)
    assert receipt.version > workspace.initial_version
    assert (reader.path / "file.txt").read_bytes() == b"published\n"
    assert (reader.path / ".git").is_file()
    assert (workspace.source / "file.txt").read_bytes() == b"original\n"
    assert workspace.result(receipt.request) == receipt
    opened = Workspace.open(
        source=workspace.source,
        authority=workspace.metadata.authority,
        binary=workspace.metadata.binary,
    )
    assert opened.binding(reader.leaf).version == receipt.version
    added_later = opened.add(tmp_path / "later")
    assert (added_later.path / "file.txt").read_bytes() == b"published\n"


def test_sync_refuses_to_destroy_uncaptured_edits(workspace: Workspace, tmp_path: Path) -> None:
    writer = workspace.add(tmp_path / "writer")
    reader = workspace.add(tmp_path / "reader")
    grant = workspace.activate(workspace.acquire(writer, ["file.txt"])[0])
    (writer.path / "file.txt").write_bytes(b"published\n")
    workspace.capture([grant])
    workspace.publish(RequestId(writer.leaf, 1), ["file.txt"])
    (reader.path / "file.txt").write_bytes(b"unrecorded local edit\n")
    with pytest.raises(MetadataError) as error:
        workspace.sync(reader)
    assert error.value.code == "dirty_path"
    assert (reader.path / "file.txt").read_bytes() == b"unrecorded local edit\n"


def test_stale_installed_origin_requires_sync(workspace: Workspace, tmp_path: Path) -> None:
    writer = workspace.add(tmp_path / "writer")
    reader = workspace.add(tmp_path / "reader")
    grant = workspace.activate(workspace.acquire(writer, ["file.txt"])[0])
    (writer.path / "file.txt").write_bytes(b"published\n")
    workspace.capture([grant])
    workspace.publish(RequestId(writer.leaf, 1), ["file.txt"])
    workspace.release(workspace.acquire(writer, ["file.txt"])[0])
    with pytest.raises(MetadataError) as error:
        workspace.acquire(reader, ["file.txt"])
    assert error.value.code == "tree_outdated"
    assert error.value.retry_action is RetryAction.SYNC_WORKSPACE
    workspace.sync(reader)
    acquired = workspace.activate(workspace.acquire(reader, ["file.txt"])[0])
    assert acquired.origin is not None


def test_delete_executable_and_symlink_survive_publication(
    workspace: Workspace,
    tmp_path: Path,
) -> None:
    writer = workspace.add(tmp_path / "writer")
    reader = workspace.add(tmp_path / "reader")
    grants = tuple(
        workspace.activate(grant)
        for grant in workspace.acquire(writer, ["file.txt", "run.sh", "link"])
    )
    (writer.path / "file.txt").unlink()
    (writer.path / "run.sh").write_bytes(b"#!/bin/sh\nexit 0\n")
    (writer.path / "run.sh").chmod(0o755)
    (writer.path / "link").symlink_to("run.sh")
    workspace.capture(grants)
    workspace.publish(RequestId(writer.leaf, 1), [grant.path for grant in grants])
    (reader.path / "untracked").write_bytes(b"keep me")
    workspace.sync(reader)
    assert not (reader.path / "file.txt").exists()
    assert (reader.path / "run.sh").stat().st_mode & 0o111
    assert os.readlink(reader.path / "link") == "run.sh"
    assert (reader.path / "untracked").read_bytes() == b"keep me"


def test_remove_keeps_metadata_when_git_rejects_dirty_worktree(
    workspace: Workspace,
    tmp_path: Path,
) -> None:
    leaf = workspace.add(tmp_path / "writer")
    (leaf.path / "file.txt").write_bytes(b"dirty\n")
    with pytest.raises(MetadataError) as error:
        workspace.remove(leaf)
    assert error.value.code == "command_failed"
    assert workspace.binding(leaf.leaf).path == leaf.path
    workspace.remove(leaf, force=True)
    assert not leaf.path.exists()
    with pytest.raises(MetadataError) as error:
        workspace.binding(leaf.leaf)
    assert error.value.code in ("leaf_inactive", "unbound_leaf")


def test_post_capture_edits_survive_publication_and_sync(
    workspace: Workspace,
    tmp_path: Path,
) -> None:
    writer = workspace.add(tmp_path / "writer")
    reader = workspace.add(tmp_path / "reader")
    grant = workspace.activate(workspace.acquire(writer, ["file.txt"])[0])
    (writer.path / "file.txt").write_bytes(b"captured A\n")
    workspace.capture([grant])
    request = RequestId(writer.leaf, 1)
    workspace.propose(request, ["file.txt"])
    candidate = workspace.prepare(request)
    (writer.path / "file.txt").write_bytes(b"later B\n")
    workspace.capture([grant])
    receipt = workspace.commit_candidate(candidate)
    workspace.sync(writer, receipt.version)
    workspace.sync(reader, receipt.version)
    assert (writer.path / "file.txt").read_bytes() == b"later B\n"
    assert (reader.path / "file.txt").read_bytes() == b"captured A\n"


@pytest.mark.parametrize(
    "boundary",
    [
        "after-install-intent",
        "before-install-rename",
        "after-install-path",
        "before-install-ack",
        "after-install-ack",
    ],
)
def test_interrupted_physical_install_recovers_from_durable_intent(
    workspace: Workspace,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    boundary: str,
) -> None:
    writer = workspace.add(tmp_path / "writer")
    reader = workspace.add(tmp_path / "reader")
    grants = tuple(
        workspace.activate(grant) for grant in workspace.acquire(writer, ["file.txt", "second.txt"])
    )
    (writer.path / "file.txt").write_bytes(b"new first\n")
    (writer.path / "second.txt").write_bytes(b"new second\n")
    workspace.capture(grants)
    receipt = workspace.publish(RequestId(writer.leaf, 1), [grant.path for grant in grants])
    with monkeypatch.context() as isolated:
        isolated.setenv("COWTREE_CRASH_AT", boundary)
        with pytest.raises(MetadataError) as error:
            workspace.sync(reader, receipt.version)
        assert error.value.code == "metadata_process"
    opened = Workspace.open(
        source=workspace.source,
        authority=workspace.metadata.authority,
        binary=workspace.metadata.binary,
    )
    recovered = opened.recover(reader)
    assert recovered.version == receipt.version
    assert recovered.pending is None
    assert (reader.path / "file.txt").read_bytes() == b"new first\n"
    assert (reader.path / "second.txt").read_bytes() == b"new second\n"
    assert (reader.path / ".git").is_file()


@pytest.mark.parametrize("boundary", ["after-import-manifest", "after-import-payloads"])
def test_interrupted_initialization_resumes_recorded_source(
    cow_repository: Repository,
    metadata_binary: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    boundary: str,
) -> None:
    authority = tmp_path / "authority"
    with monkeypatch.context() as isolated:
        isolated.setenv("COWTREE_CRASH_AT", boundary)
        with pytest.raises(MetadataError) as error:
            Workspace.create(
                source=cow_repository.path, authority=authority, binary=metadata_binary
            )
        assert error.value.code == "metadata_process"
    workspace = Workspace.open(
        source=cow_repository.path,
        authority=authority,
        binary=metadata_binary,
    )
    assert workspace.initial_version == 1
    leaf = workspace.add(tmp_path / "resumed")
    assert (leaf.path / "file.txt").read_bytes() == b"original\n"


@pytest.mark.parametrize("boundary", ["before-authority-publish", "after-authority-publish"])
def test_initialization_publishes_authority_and_config_together(
    cow_repository: Repository,
    metadata_binary: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    boundary: str,
) -> None:
    authority = tmp_path / "authority"
    with monkeypatch.context() as isolated:
        isolated.setenv("COWTREE_CRASH_AT", boundary)
        with pytest.raises(MetadataError) as error:
            Workspace.create(
                source=cow_repository.path, authority=authority, binary=metadata_binary
            )
        assert error.value.code == "metadata_process"
    if boundary == "before-authority-publish":
        assert not authority.exists()
        workspace = Workspace.create(
            source=cow_repository.path,
            authority=authority,
            binary=metadata_binary,
        )
    else:
        assert (authority / "workspace.json").is_file()
        workspace = Workspace.open(
            source=cow_repository.path,
            authority=authority,
            binary=metadata_binary,
        )
    assert workspace.initial_version == 1
    assert workspace.tip().version == 1
    assert workspace.leaves() == ()


def test_managed_writers_publish_one_atomic_batch(workspace: Workspace, tmp_path: Path) -> None:
    first = workspace.add(tmp_path / "first")
    second = workspace.add(tmp_path / "second")
    reader = workspace.add(tmp_path / "reader")
    first_grant = workspace.activate(workspace.acquire(first, ["file.txt"])[0])
    second_grant = workspace.activate(workspace.acquire(second, ["second.txt"])[0])
    (first.path / "file.txt").write_bytes(b"first writer\n")
    (second.path / "second.txt").write_bytes(b"second writer\n")
    workspace.capture([first_grant])
    workspace.capture([second_grant])
    requests = (RequestId(first.leaf, 1), RequestId(second.leaf, 1))
    workspace.propose(requests[0], ["file.txt"])
    workspace.propose(requests[1], ["second.txt"])
    with pytest.raises(MetadataError) as error:
        workspace.prepare_batch(requests, expected_tip=0)
    assert error.value.code == "tip_changed"
    assert error.value.retry_action is RetryAction.REPREPARE
    candidate = workspace.prepare_batch(requests, expected_tip=workspace.initial_version)
    result = workspace.commit_batch(candidate)
    assert len(result.receipts) == 2
    assert {receipt.version for receipt in result.receipts} == {workspace.initial_version + 1}
    assert len({receipt.root for receipt in result.receipts}) == 1
    assert workspace.commit_batch(candidate) == result
    assert all(workspace.result(receipt.request) == receipt for receipt in result.receipts)
    workspace.sync(reader, result.receipts[0].version)
    assert (reader.path / "file.txt").read_bytes() == b"first writer\n"
    assert (reader.path / "second.txt").read_bytes() == b"second writer\n"


def test_explicit_resolution_keeps_later_physical_edits(
    workspace: Workspace,
    tmp_path: Path,
) -> None:
    writer = workspace.add(tmp_path / "writer")
    reader = workspace.add(tmp_path / "reader")
    grant = workspace.activate(workspace.acquire(writer, ["file.txt"])[0])
    first = RequestId(writer.leaf, 1)
    second = RequestId(writer.leaf, 2)
    resolved = RequestId(writer.leaf, 3)
    (writer.path / "file.txt").write_bytes(b"choice A\n")
    workspace.capture([grant])
    workspace.propose(first, ["file.txt"])
    (writer.path / "file.txt").write_bytes(b"choice B\n")
    workspace.capture([grant])
    workspace.propose(second, ["file.txt"])
    (writer.path / "file.txt").write_bytes(b"later C\n")
    workspace.capture([grant])
    with pytest.raises(MetadataError) as error:
        workspace.prepare_batch([first, second], expected_tip=workspace.initial_version)
    assert error.value.code == "batch_conflict"
    assert error.value.retry_action is RetryAction.NONE
    with pytest.raises(MetadataError) as error:
        workspace.resolve(resolved, [first, second], choices={})
    assert error.value.code == "resolution_conflict"
    assert error.value.retry_action is RetryAction.NONE
    proposal = workspace.resolve(resolved, [first, second], choices={"file.txt": first})
    assert workspace.resolve(resolved, [first, second], choices={"file.txt": first}) == proposal
    with pytest.raises(MetadataError) as error:
        workspace.prepare(first)
    assert error.value.code == "aborted"
    receipt = workspace.commit_candidate(workspace.prepare(resolved))
    workspace.sync(writer, receipt.version)
    workspace.sync(reader, receipt.version)
    assert (writer.path / "file.txt").read_bytes() == b"later C\n"
    assert (reader.path / "file.txt").read_bytes() == b"choice A\n"


@dataclass
class PausedInstallation:
    process: subprocess.Popen[bytes]
    marker: Path

    @classmethod
    def start(cls, workspace: Workspace, leaf: WorkspaceLeaf, marker: Path) -> PausedInstallation:
        env = dict(os.environ)
        env["COWTREE_PAUSE_AT"] = "after-install-intent"
        env["COWTREE_PAUSE_FILE"] = str(marker)
        request = marker.with_suffix(".jsonl")
        request.write_text(json.dumps({"op": "install", "leaf": leaf.leaf, "version": 1}) + "\n")
        with request.open("rb") as source:
            process = subprocess.Popen(
                args=[str(workspace.metadata.binary), str(workspace.metadata.authority)],
                stdin=source,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=env,
            )
        deadline = time.monotonic() + 10
        while not marker.exists():
            assert process.poll() is None, "installer exited before its pause boundary"
            assert time.monotonic() < deadline, "installer did not reach its pause boundary"
            time.sleep(0.01)
        result = cls(process=process, marker=marker)
        return result

    def resume(self) -> None:
        self.marker.unlink(missing_ok=True)
        stdout, stderr = self.process.communicate(timeout=10)
        assert self.process.returncode == 0, stderr.decode()
        assert json.loads(stdout)["status"] == "ok", stdout.decode()


def test_remove_waits_for_cooperating_installer(workspace: Workspace, tmp_path: Path) -> None:
    leaf = workspace.add(tmp_path / "reader")
    paused = PausedInstallation.start(workspace, leaf, tmp_path / "paused")
    started = threading.Event()

    def remove() -> None:
        started.set()
        workspace.remove(leaf, force=True)

    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
        removal = executor.submit(remove)
        try:
            assert started.wait(timeout=1)
            with pytest.raises(concurrent.futures.TimeoutError):
                removal.result(timeout=0.25)
            assert leaf.path.exists()
        finally:
            paused.resume()
        removal.result(timeout=10)
    assert not leaf.path.exists()
    assert leaf.leaf not in workspace.leaves()


def test_remove_preserves_pending_install_until_recovery(
    workspace: Workspace,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    leaf = workspace.add(tmp_path / "reader")
    with monkeypatch.context() as isolated:
        isolated.setenv("COWTREE_CRASH_AT", "after-install-intent")
        with pytest.raises(MetadataError) as error:
            workspace.sync(leaf)
        assert error.value.code == "metadata_process"
    with pytest.raises(MetadataError) as error:
        workspace.remove(leaf, force=True)
    assert error.value.code == "needs_recovery"
    assert leaf.path.exists()
    workspace.recover(leaf)
    workspace.remove(leaf, force=True)
    assert not leaf.path.exists()
    assert leaf.leaf not in workspace.leaves()


def test_remove_lock_failure_preserves_worktree(workspace: Workspace, tmp_path: Path) -> None:
    leaf = workspace.add(tmp_path / "reader")
    lock = workspace.metadata.authority / "workspace.lock"
    lock.unlink()
    lock.mkdir()
    with pytest.raises(MetadataError) as error:
        workspace.remove(leaf, force=True)
    assert error.value.code == "workspace_io"
    assert leaf.path.exists()
    lock.rmdir()
    workspace.remove(leaf, force=True)
    assert not leaf.path.exists()


def test_remove_invalid_path_releases_authority_lock(workspace: Workspace, tmp_path: Path) -> None:
    leaf = workspace.add(tmp_path / "reader")
    invalid = WorkspaceLeaf(leaf=leaf.leaf, path=Path("\0"))
    with pytest.raises(MetadataError) as error:
        workspace.remove(invalid, force=True)
    assert error.value.code == "invalid_arguments"
    assert leaf.path.exists()
    workspace.remove(leaf, force=True)
    assert not leaf.path.exists()


def test_stale_initializer_reads_ready_config_without_filesystem_reentry(
    workspace: Workspace,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    stale = WireRecord(
        {
            "format": 1,
            "source": str(workspace.source),
            "commit": workspace.commit,
            "initial_version": None,
        }
    )
    original = MetadataClient.call

    def read_only(client: MetadataClient, command: dict[str, Json], kind: str) -> Json:
        assert command["op"] == "tip", "stale initializer tried to reacquire the filesystem lock"
        result = original(client, command, kind)
        return result

    monkeypatch.setattr(MetadataClient, "call", read_only)
    with workspace_lock(authority=workspace.metadata.authority):
        version = finish_import(metadata=workspace.metadata, source=workspace.source, config=stale)
    assert version == workspace.initial_version


@pytest.mark.parametrize(
    ("key", "value"), [("format", 2), ("source", "/changed"), ("commit", "0" * 40)]
)
def test_stale_initializer_rejects_changed_identity(
    workspace: Workspace,
    key: str,
    value: Json,
) -> None:
    fields: dict[str, Json] = {
        "format": 1,
        "source": str(workspace.source),
        "commit": workspace.commit,
        "initial_version": None,
    }
    current = dict(fields)
    current["initial_version"] = workspace.initial_version
    current[key] = value
    (workspace.metadata.authority / "workspace.json").write_text(json.dumps(current))
    with pytest.raises(MetadataError) as error:
        finish_import(
            metadata=workspace.metadata, source=workspace.source, config=WireRecord(fields)
        )
    assert error.value.code == "workspace_configuration"
    assert workspace.tip().version == workspace.initial_version
