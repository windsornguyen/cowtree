"""Check model observations against real leases, frozen proposals, and file bytes."""

# TLC's JSON is retained byte for byte. The manifest supplies reviewed API actions;
# post-state expectations always come from the checker, never from the adapter.
from enum import Enum
import hashlib
from pathlib import Path
import sys
from typing import Literal

from pydantic import Field, JsonValue, TypeAdapter
import pytest

from cowtree.captures import Captures
from cowtree.checks import Checks
from cowtree.exec import CommandRunner
from cowtree.leaves import Leaves
from cowtree.metadata_types import Entry, Grant, Operation, Proposal, Record, Tip
from cowtree.publications import Publications
from cowtree.tree_types import PathPolicy
from cowtree.views import Views
from cowtree.workspace import Workspace

from .test_bootstrap import metadata_binary, source_repository
from .test_fork import manager


class Action(str, Enum):
    FORK = "fork"
    ACQUIRE = "acquire"
    WRITE = "write"
    PROPOSE = "propose"
    COMMIT = "commit"
    SYNC = "sync"
    EXPIRE = "expire"
    DISCARD = "discard"


class Step(Record):
    action: Action
    leaf: str
    path: str = "p1"
    value: str = "v1"


class TraceManifest(Record):
    schema_version: Literal[1]
    checker_version: Literal["1.8.0"]
    checker_sha256: str
    inputs: dict[str, str]
    export_sha256: str
    actions: list[Step]


class ModelProposal(Record):
    leaf: str
    delta: dict[str, str]
    tokens: dict[str, int]
    origin: dict[str, str]
    base: int
    stale: list[str]


class ModelState(Record):
    origin: dict[str, dict[str, str]]
    active: dict[str, bool]
    dirty: dict[str, list[str]]
    log: list[dict[str, str]]
    rogue: bool
    stale_base_committed: bool = Field(alias="staleBaseCommitted")
    view: dict[str, dict[str, str]]
    holder: dict[str, str]
    token: dict[str, int]
    merges: list[JsonValue]
    proposals: list[ModelProposal]
    expired: bool
    stale: dict[str, list[str]]
    read_epoch: dict[str, int] = Field(alias="readEpoch")
    held: dict[str, dict[str, int]]
    step: int = 0


class Counterexample(Record):
    action: list[list[JsonValue]]
    state: list[tuple[int, ModelState]]


class Export(Record):
    counterexample: Counterexample
    vars: list[str]


def checked_trace(name: str) -> tuple[TraceManifest, Export]:
    root = Path(__file__).resolve().parents[1]
    directory = root / "tests/traces"
    manifest = TraceManifest.model_validate_json((directory / f"{name}.manifest.json").read_bytes())
    from scripts.check_specs import TLC_SHA256, TLC_VERSION

    assert manifest.checker_sha256 == TLC_SHA256
    assert manifest.checker_version == TLC_VERSION
    for path, digest in manifest.inputs.items():
        assert path.startswith("specs/")
        assert ".." not in Path(path).parts
        assert hashlib.sha256((root / path).read_bytes()).hexdigest() == digest, path
    raw = (directory / f"{name}.json").read_bytes()
    assert hashlib.sha256(raw).hexdigest() == manifest.export_sha256
    exported = Export.model_validate_json(raw)
    assert len(exported.counterexample.state) == len(manifest.actions) + 1
    assert len(exported.counterexample.action) == len(manifest.actions)
    return manifest, exported


def symbolic_entry(value: str) -> Entry:
    from cowtree.metadata_types import EntryKind

    result = Entry(object=hashlib.sha256(value.encode()).hexdigest(), kind=EntryKind.FILE)
    return result


def assert_observation(leaves: Leaves, identities: dict[str, int], state: ModelState) -> None:
    assert set(identities) == {name for name, active in state.active.items() if active}
    with leaves.workspace.session() as metadata:
        tip = metadata.call(Operation.TIP).decode("tip", TypeAdapter(Tip))
        # Workspace import appends its initial tree to metadata's empty version zero.
        assert tip.version == len(state.log)
        for version, expected in enumerate(state.log, start=1):
            snapshot = metadata.call(Operation.SNAPSHOT, {"version": version}).decode(
                "snapshot", TypeAdapter(dict[str, Entry])
            )
            assert {key: snapshot[key] for key in expected} == {
                key: symbolic_entry(value) for key, value in expected.items()
            }
        live = metadata.call(Operation.GRANTS).decode("grants", TypeAdapter(list[Grant]))
        assert all(grant.activated for grant in live)
        assert len({grant.token for grant in live}) == len(live)
        assert {grant.path: grant.leaf for grant in live} == {
            path: identities[holder] for path, holder in state.holder.items() if holder != "None"
        }
    for name, identity in identities.items():
        leaf = leaves.read(identity=identity)
        expected = state.view[name]
        assert {path: (leaf.path / path).read_text() for path in expected} == expected
        assert {path: leaf.origins[path] for path in expected} == {
            path: symbolic_entry(value) for path, value in state.origin[name].items()
        }
        proposals = [proposal for proposal in state.proposals if proposal.leaf == name]
        if proposals:
            assert len(proposals) == 1
            assert leaf.pending is not None
            assert leaf.pending.submitted
            proposal = proposals[0]
            assert leaf.pending.changes == {
                path: symbolic_entry(value)
                for path, value in proposal.delta.items()
                if value != "None"
            }
            with leaves.workspace.session() as metadata:
                captured = metadata.call(
                    Operation.PROPOSE,
                    {
                        "input": {
                            "request": leaf.pending.request.model_dump(mode="json"),
                            "paths": sorted(leaf.pending.changes),
                        }
                    },
                ).decode("proposal", TypeAdapter(Proposal))
            assert captured.request == leaf.pending.request
            assert {
                change.path: change.value for change in captured.changes
            } == leaf.pending.changes
            for change in captured.changes:
                assert change.token == leaf.grants[change.path].token
                assert change.origin == symbolic_entry(proposal.origin[change.path])
        else:
            assert leaf.pending is None
        baseline: dict[str, Entry | None] = dict(leaf.origins)
        if leaf.pending is not None:
            baseline.update(leaf.pending.changes)
        dirty = {
            path for path, value in expected.items() if symbolic_entry(value) != baseline[path]
        }
        assert dirty == set(state.dirty[name])


def run_action(leaves: Leaves, identities: dict[str, int], step: Step) -> None:
    if step.action is Action.FORK:
        leaf = leaves.fork(path=leaves.workspace.root.parent / step.leaf)
        identities[step.leaf] = leaf.id
        return
    identity = identities[step.leaf]
    views = Views(leaves.workspace)
    match step.action:
        case Action.ACQUIRE:
            views.acquire(identity=identity, paths=(step.path,))
        case Action.WRITE:
            (leaves.read(identity=identity).path / step.path).write_text(step.value)
        case Action.PROPOSE:
            assert Captures(leaves.workspace).capture(identity=identity) is not None
        case Action.SYNC:
            views.sync(identity=identity)
        case Action.EXPIRE:
            with leaves.workspace.session() as metadata:
                metadata.call(Operation.REVOKE, {"path": step.path})
        case Action.DISCARD:
            views.discard(identity=identity, paths=(step.path,))
        case Action.COMMIT:
            publications = Publications(leaves.workspace)
            candidate = publications.prepare(identity=identity)
            Checks(leaves.workspace).validate(
                identity=identity, command=(sys.executable, "-c", "pass")
            )
            receipt = publications.commit(candidate=candidate)
            assert publications.result(request=candidate.request) == receipt
            assert publications.commit(candidate=candidate) == receipt
            # EpochLog.Commit also installs unrelated clean paths atomically.
            # The runtime deliberately exposes that installation as a separate sync.
            views.sync(identity=identity)
        case _:
            raise AssertionError(f"unhandled trace action: {step.action}")


@pytest.mark.parametrize(
    "name", ["EpochLogWitness", "EpochLogReplayDiscard", "EpochLogReplayCommit"]
)
def test_tlc_trace_against_filesystem_and_sqlite(tmp_path: Path, name: str) -> None:
    manifest, exported = checked_trace(name=name)
    source = source_repository(path=tmp_path / "source")
    initial = exported.counterexample.state[0][1]
    for path, value in initial.log[0].items():
        (source / path).write_text(value)
    runner = CommandRunner()
    runner.run(["git", "-C", str(source), "add", "."])
    runner.run(["git", "-C", str(source), "commit", "-qm", "model keys"])
    leaves = Leaves(
        Workspace.create(
            root=tmp_path / "workspace",
            source=source,
            binary=metadata_binary(),
            policy=PathPolicy(derived=("cache",)),
        )
    )
    identities: dict[str, int] = {}
    assert_observation(leaves=leaves, identities=identities, state=initial)
    for number, action in enumerate(manifest.actions, start=1):
        run_action(leaves=leaves, identities=identities, step=action)
        state_number, expected = exported.counterexample.state[number]
        assert state_number == number + 1
        assert_observation(leaves=leaves, identities=identities, state=expected)


def test_discard_restores_pending_bytes_without_canceling_the_request(tmp_path: Path) -> None:
    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "writer")
    path = leaf.path / "file.txt"
    path.write_bytes(b"submitted")
    pending = Captures(leaves.workspace).capture(identity=leaf.id)
    path.write_bytes(b"later edit")
    (leaf.path / "unrelated").write_bytes(b"keep")
    updated = Views(leaves.workspace).discard(identity=leaf.id, paths=("file.txt",))
    assert path.read_bytes() == b"submitted"
    assert (leaf.path / "unrelated").read_bytes() == b"keep"
    assert updated.pending == pending
    assert updated.origins == leaf.origins
    assert list((leaves.workspace.root / "operations").iterdir()) == []


def test_sync_rejects_a_private_parent_before_writing_intent(tmp_path: Path) -> None:
    from cowtree.errors import CowtreeError

    from .test_views import publish_remote

    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "reader")
    (leaf.path / "private").write_bytes(b"keep")
    publish_remote(leaves=leaves, path="private/child", data=b"published")
    with pytest.raises(CowtreeError):
        Views(leaves.workspace).sync(identity=leaf.id)
    assert (leaf.path / "private").read_bytes() == b"keep"
    assert list((leaves.workspace.root / "operations").iterdir()) == []


def test_discard_recovers_an_interrupted_installation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from cowtree.install import Installation

    leaves = manager(root=tmp_path)
    leaf = leaves.fork(path=tmp_path / "writer")
    target = leaf.path / "file.txt"
    target.write_bytes(b"submitted")
    pending = Captures(leaves.workspace).capture(identity=leaf.id)
    target.write_bytes(b"later")
    original = Installation.apply

    def interrupt(self: Installation, *, rollback: bool = False) -> None:
        original(self, rollback=rollback)
        raise InterruptedError("installed before acknowledgement")

    with monkeypatch.context() as patch:
        patch.setattr(Installation, "apply", interrupt)
        with pytest.raises(InterruptedError):
            Views(leaves.workspace).discard(identity=leaf.id, paths=("file.txt",))
    with leaves.workspace.session():
        assert target.read_bytes() == b"submitted"
        assert leaves.read(identity=leaf.id).pending == pending
    assert list((leaves.workspace.root / "operations").iterdir()) == []
