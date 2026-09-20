"""Pinned dependencies have private bytes and immutable publication identities."""

import os
from pathlib import Path
import shutil
import sys

from pydantic import TypeAdapter
import pytest

from cowtree.captures import Captures
from cowtree.checks import Checks
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.exec import CommandRunner
from cowtree.git import GitRepository
from cowtree.leaves import Leaves
from cowtree.lifecycle import Lifecycle
from cowtree.metadata import MetadataError
from cowtree.metadata_types import Entry, MetadataReason, Operation, ReadOnlyKind, Tip
from cowtree.publications import Publications
from cowtree.submodule_types import SubmodulePolicy
from cowtree.tree_types import PathPolicy
from cowtree.views import Views
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary, source_repository


def source_tree(root: Path) -> Path:
    source = source_repository(root / "source")
    dependency = source_repository(root / "dependency")
    repository = GitRepository(io=CommandRunner(), path=source)
    repository.run(
        ["-c", "protocol.file.allow=always", "submodule", "add", str(dependency), "vendor/dep"]
    )
    repository.run(["commit", "-qam", "pin dependency"])
    child = GitRepository(io=repository.io, path=source / "vendor/dep")
    child.run(["config", "user.name", "Test"])
    child.run(["config", "user.email", "test@example.invalid"])
    return source


def test_pinned_materialization_retains_identity_without_shared_git_state(tmp_path: Path) -> None:
    source = source_tree(tmp_path)
    repository = GitRepository(io=CommandRunner(), path=source)
    child = GitRepository(io=repository.io, path=source / "vendor/dep")
    pin = child.head()
    index = repository.capture(["ls-files", "--stage", "-z"])
    root = tmp_path / "store"
    with pytest.raises(CowtreeError) as refused:
        Workspace.create(root, source, metadata_binary(), PathPolicy())
    assert refused.value.code is CowtreeErrorCode.SUBMODULE_UNSUPPORTED
    assert not root.exists()
    workspace = Workspace.create(
        root,
        source,
        metadata_binary(),
        PathPolicy(
            derived=("cache",),
            submodules=SubmodulePolicy.MATERIALIZE_PINNED,
        ),
    )
    leaves = Leaves(workspace)
    first = leaves.fork(tmp_path / "first")
    sibling = leaves.fork(tmp_path / "sibling")
    assert workspace.config.policy.pins[0].commit == pin
    assert not (first.path / "vendor/dep/.git").exists()
    assert not (sibling.path / "vendor/dep/.git").exists()
    assert (first.path / "cache/artifact").read_bytes() == b"warm build"
    (first.path / "file.txt").write_bytes(b"published parent edit\n")
    assert Captures(workspace).capture(first.id) is not None
    candidate = Publications(workspace).prepare(first.id)
    Checks(workspace).validate(first.id, (sys.executable, "-c", "pass"))
    Publications(workspace).commit(candidate)
    with workspace.session() as metadata:
        tip = metadata.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
        snapshot = metadata.call(Operation.SNAPSHOT, {"version": tip.version}).decode(
            "snapshot",
            TypeAdapter(dict[str, Entry]),
        )
    assert isinstance(snapshot["vendor/dep/file.txt"].kind, ReadOnlyKind)
    assert snapshot["vendor/dep/file.txt"].kind.read_only.scope == "vendor/dep"
    (first.path / "vendor/dep/file.txt").write_bytes(b"private changed bytes")
    assert (sibling.path / "vendor/dep/file.txt").read_bytes() == b"source bytes\n"
    assert (source / "vendor/dep/file.txt").read_bytes() == b"source bytes\n"
    with pytest.raises(CowtreeError) as changed:
        Captures(workspace).capture(first.id)
    assert changed.value.code is CowtreeErrorCode.DEPENDENCY_CHANGED
    with pytest.raises(MetadataError) as denied:
        Views(workspace).acquire(sibling.id, ("vendor",))
    assert denied.value.reason is MetadataReason.READ_ONLY_PATH
    leaves.recover()
    Lifecycle(workspace).drop(first.id, force=True)
    Lifecycle(workspace).drop(sibling.id, force=True)
    assert child.head() == pin
    assert repository.capture(["ls-files", "--stage", "-z"]) == index


@pytest.mark.parametrize(
    "damage",
    [
        "dirty",
        "untracked",
        "ignored",
        "missing",
        "mismatch",
        "nested",
        "reserved",
        "absolute-link",
        "external-link",
        "cycle",
    ],
)
def test_unsupported_dependency_states_fail_before_workspace_reservation(
    tmp_path: Path, damage: str
) -> None:
    source = source_tree(tmp_path)
    parent = GitRepository(io=CommandRunner(), path=source)
    child = GitRepository(io=parent.io, path=source / "vendor/dep")
    if damage == "dirty":
        (child.path / "file.txt").write_bytes(b"changed")
    elif damage == "untracked":
        (child.path / "extra").write_bytes(b"extra")
    elif damage == "ignored":
        (child.path / "cache").mkdir()
        (child.path / "cache/generated").write_bytes(b"ignored")
    elif damage == "missing":
        shutil.rmtree(child.path)
    elif damage == "mismatch":
        child.run(["commit", "--allow-empty", "-qm", "different tip"])
    else:
        if damage == "nested":
            nested = source_repository(tmp_path / "nested")
            child.run(
                ["-c", "protocol.file.allow=always", "submodule", "add", str(nested), "nested"]
            )
        elif damage == "reserved":
            assert damage == "reserved"
            (child.path / ".cowtree-pin").write_bytes(b"reserved")
            child.run(["add", ".cowtree-pin"])
        else:
            targets = {
                "absolute-link": str(child.path / "file.txt"),
                "external-link": "../../file.txt",
                "cycle": "alias",
            }
            (child.path / "alias").symlink_to(targets[damage])
            child.run(["add", "alias"])
        child.run(["commit", "-qam", "changed dependency shape"])
        parent.run(["add", "vendor/dep"])
        parent.run(["commit", "-qm", "pin changed shape"])
    root = tmp_path / "store"
    with pytest.raises(CowtreeError):
        Workspace.create(
            root,
            source,
            metadata_binary(),
            PathPolicy(submodules=SubmodulePolicy.MATERIALIZE_PINNED),
        )
    assert not root.exists()
    assert not Workspace.staging(root).exists()


def test_index_stat_caches_cannot_hide_a_changed_dependency(tmp_path: Path) -> None:
    source = source_tree(tmp_path)
    child = GitRepository(io=CommandRunner(), path=source / "vendor/dep")
    child.run(["config", "core.trustctime", "false"])
    child.run(["config", "core.checkStat", "minimal"])
    path = child.path / "file.txt"
    timestamp = 1_700_000_000_000_000_000
    os.utime(path, ns=(timestamp, timestamp))
    child.run(["update-index", "--refresh"])
    path.write_bytes(b"hidden bytes\n")
    os.utime(path, ns=(timestamp, timestamp))
    assert child.status() == ""
    root = tmp_path / "store"
    with pytest.raises(CowtreeError) as caught:
        Workspace.create(
            root,
            source,
            metadata_binary(),
            PathPolicy(submodules=SubmodulePolicy.MATERIALIZE_PINNED),
        )
    assert caught.value.code is CowtreeErrorCode.DIRTY_SOURCE
    assert not Workspace.staging(root).exists()
