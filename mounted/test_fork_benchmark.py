"""Fork timing excludes byte verification and preserves preexisting owned leaves."""

import os
from pathlib import Path

import pytest

from benchmarks.cargo_workspace import inventory, verify
from benchmarks.fork_workspace import Config, Report, run
from cowtree.errors import CowtreeError
from cowtree.leaves import Leaves
from cowtree.metadata_types import Candidate
from cowtree.tree_types import PathPolicy
from cowtree.workspace import Workspace
from cowtree.workspace_types import Leaf

from .test_bootstrap import metadata_binary, source_repository


def test_fork_benchmark_verifies_bytes_modes_times_and_empty_directories(tmp_path: Path) -> None:
    source = source_repository(path=tmp_path / "source")
    (source / "cache/empty/nested").mkdir(parents=True)
    (source / "cache/artifact").chmod(0o640)
    workspace = Workspace.create(
        root=tmp_path / "store",
        source=source,
        binary=metadata_binary(),
        policy=PathPolicy(derived=("cache",)),
    )
    leaves = Leaves(workspace)
    sibling = leaves.fork(path=tmp_path / "sibling")
    before = inventory(root=sibling.path)
    config = Config(
        store=workspace.root, node=workspace.config.initial, root=tmp_path / "timing", trials=2
    )
    report = run(config=config)
    assert len(report.trials) == 2
    assert report.source_directories >= 3
    assert report.verified_files >= report.source_entries * 3
    assert report.fresh_median_seconds > 0
    assert report.prepared_lookup_median_seconds > 0
    assert report.prepared_lookup_excludes_creation
    assert not report.atomic_worker_claim
    assert leaves.records() == [sibling]
    verify(root=sibling.path, expected=before)
    assert list(config.root.glob("trial-*")) == []
    assert Report.model_validate_json((config.root / "report.json").read_bytes()) == report
    with pytest.raises(CowtreeError, match="destination exists"):
        run(config=config)
    with pytest.raises(CowtreeError, match="cannot nest"):
        run(config=config.model_copy(update={"root": sibling.path / "benchmark"}))


@pytest.mark.parametrize("destination", ["leaf", "source", "sibling"])
@pytest.mark.parametrize("damage", ["mode", "mtime"])
def test_fork_benchmark_refuses_directory_only_corruption(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, destination: str, damage: str
) -> None:
    source = source_repository(path=tmp_path / "source")
    (source / "cache/empty").mkdir(mode=0o750)
    workspace = Workspace.create(
        root=tmp_path / "store",
        source=source,
        binary=metadata_binary(),
        policy=PathPolicy(derived=("cache",)),
    )
    sibling = Leaves(workspace).fork(path=tmp_path / "sibling")
    original = Leaves.fork

    def corrupt(
        self: Leaves, path: Path, node: str | None = None, check_candidate: Candidate | None = None
    ) -> Leaf:
        leaf = original(self, path=path, node=node, check_candidate=check_candidate)
        roots = {"leaf": leaf.path, "source": source, "sibling": sibling.path}
        directory = roots[destination] / "cache/empty"
        metadata = directory.stat()
        if damage == "mode":
            directory.chmod(0o777)
        else:
            assert damage == "mtime"
            os.utime(directory, ns=(metadata.st_atime_ns, metadata.st_mtime_ns + 1_000_000_000))
        return leaf

    monkeypatch.setattr(Leaves, "fork", corrupt)
    config = Config(
        store=workspace.root, node=workspace.config.initial, root=tmp_path / "timing", trials=1
    )
    with pytest.raises(CowtreeError, match="directory names, modes, or times differ"):
        run(config=config)
    assert not (config.root / "report.json").exists()
    assert Leaves(workspace).records() == [sibling]
