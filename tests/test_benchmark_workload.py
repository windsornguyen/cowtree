"""Exercise the seeded benchmark oracle against real Git and native CoW trees."""

from __future__ import annotations

from dataclasses import asdict
import json
import os
from pathlib import Path
import stat

import pytest

from benchmarks.churn_workload import (
    EditMode,
    Fleet,
    FleetConfig,
    Method,
    MutationConfig,
    MutationStats,
    OracleError,
)
from cowtree.core import inspect_path

from .conftest import Repository


def checkout_fixture(repository: Repository) -> None:
    """Recreate owned fixture files using Git's modes and the current umask."""
    tracked = repository.git("ls-files", "-z").stdout.split("\0")
    for name in tracked:
        if name:
            (repository.path / name).unlink()
    repository.git("checkout-index", "--all")


def create_source(repository: Repository) -> Repository:
    for index in range(12):
        (repository.path / f"source-{index}.c").write_text(f"int value_{index} = {index};\n")
    (repository.path / "line\nname.h").write_bytes(b"/* unusual filename */\n")
    (repository.path / "link").symlink_to("source-0.c")
    (repository.path / "dangling").symlink_to("absent")
    executable = repository.path / "run"
    executable.write_bytes(b"#!/bin/sh\nexit 0\n")
    executable.chmod(0o755)
    (repository.path / ".gitignore").write_text("ignored\n")
    repository.commit()
    checkout_fixture(repository=repository)
    return repository


@pytest.fixture
def source(repository: Repository) -> Repository:
    result = create_source(repository=repository)
    return result


def new_fleet(source: Repository, root: Path, method: Method) -> Fleet:
    fleet = Fleet(
        config=FleetConfig(
            repo=source.path,
            root=root,
            method=method,
            seed=1729,
            history=root.with_suffix(".jsonl"),
        )
    )
    return fleet


@pytest.mark.parametrize("method", list(Method))
@pytest.mark.parametrize("mode", list(EditMode))
def test_invariant_churn_preserves_survivors_and_cleans_missing_registry(
    source: Repository,
    tmp_path: Path,
    method: Method,
    mode: EditMode,
) -> None:
    if method is Method.COWTREE and not inspect_path(path=source.path).supported:
        pytest.skip("native copy-on-write filesystem required")
    fleet = new_fleet(source=source, root=tmp_path / "fleet", method=method)
    fleet.populate(count=3)
    initial = fleet.verify()
    modified = fleet.mutate(config=MutationConfig(round_id=0, fraction=0.3, mode=mode))
    assert modified.files_edited == 12
    assert modified.bytes_written > 0
    before = fleet.verify()
    removed = fleet.remove_random(count=1, external_loss=True)
    after = fleet.verify()
    assert after.active_worktrees == 2
    assert after.changed_files == 8
    assert after.logical_bytes < before.logical_bytes
    fleet.recreate(slots=removed)
    restored = fleet.verify()
    assert restored.active_worktrees == 3
    assert restored.changed_files == 8
    assert restored.logical_bytes > initial.logical_bytes
    fleet.mutate(config=MutationConfig(round_id=1, fraction=0.3, mode=mode))
    assert fleet.verify(full=False).changed_files >= 12
    fleet.cleanup()
    assert source.registered == {source.path}
    events = [json.loads(line) for line in fleet.config.history.read_text().splitlines()]
    assert len(events) == 2 * fleet.history.operations
    assert [event["phase"] for event in events] == ["invoke", "ok"] * fleet.history.operations


def test_invariant_identical_seed_produces_identical_cross_method_state(
    source: Repository,
    tmp_path: Path,
) -> None:
    if not inspect_path(path=source.path).supported:
        pytest.skip("native copy-on-write filesystem required")
    digests: list[str] = []
    mutations: list[MutationStats] = []
    histories: list[list[str]] = []
    for method in Method:
        fleet = new_fleet(source=source, root=tmp_path / method.value, method=method)
        fleet.populate(count=3)
        mutations.append(fleet.mutate(config=MutationConfig(round_id=0, fraction=0.2)))
        slots = fleet.remove_random(count=2, external_loss=True)
        fleet.verify()
        fleet.recreate(slots=slots)
        fleet.mutate(config=MutationConfig(round_id=1, mode=EditMode.IN_PLACE_APPEND))
        digests.append(fleet.verify().state_sha256)
        fleet.cleanup()
        records: list[str] = []
        for line in fleet.config.history.read_text().splitlines():
            event = json.loads(line)
            del event["monotonic_ns"]
            records.append(json.dumps(event, sort_keys=True))
        histories.append(records)
    assert digests[0] == digests[1]
    assert mutations[0] == mutations[1]
    assert histories[0] == histories[1]


@pytest.mark.parametrize(
    "corruption", ["bytes", "mode", "permissions", "symlink", "ignored", "head"]
)
def test_invariant_oracle_rejects_unmodeled_state_changes(
    source: Repository,
    tmp_path: Path,
    corruption: str,
) -> None:
    fleet = new_fleet(source=source, root=tmp_path / "fleet", method=Method.GIT)
    fleet.populate(count=1)
    leaf = fleet.active[0]
    if corruption == "bytes":
        (leaf.path / "source-0.c").write_bytes(b"unmodeled bytes\n")
    elif corruption == "mode":
        (leaf.path / "run").chmod(0o644)
    elif corruption == "permissions":
        executable = leaf.path / "run"
        executable.chmod(stat.S_IMODE(executable.stat().st_mode) ^ stat.S_IWGRP)
    elif corruption == "symlink":
        (leaf.path / "link").unlink()
        (leaf.path / "link").symlink_to("source-1.c")
    elif corruption == "ignored":
        (leaf.path / "ignored").write_bytes(b"untracked ignored payload")
    elif corruption == "head":
        source.git("-C", str(leaf.path), "checkout", "-qb", "unexpected")
    with pytest.raises(OracleError):
        fleet.verify()
    fleet.cleanup()


def test_invariant_oracle_compares_dirty_bytes_with_prewrite_expectation(
    source: Repository,
    tmp_path: Path,
) -> None:
    fleet = new_fleet(source=source, root=tmp_path / "fleet", method=Method.GIT)
    fleet.populate(count=1)
    fleet.mutate(config=MutationConfig(round_id=0, fraction=1))
    leaf = fleet.active[0]
    name = next(iter(leaf.changed))
    (leaf.path / name).write_bytes(b"still dirty, but incorrect\n")
    with pytest.raises(OracleError, match="file differs from oracle"):
        fleet.verify(full=False)
    fleet.cleanup()


def test_invariant_source_isolation_is_checked(source: Repository, tmp_path: Path) -> None:
    fleet = new_fleet(source=source, root=tmp_path / "fleet", method=Method.GIT)
    fleet.populate(count=1)
    path = source.path / "source-0.c"
    original = path.read_bytes()
    path.write_bytes(b"source damage\n")
    with pytest.raises(OracleError):
        fleet.verify()
    path.write_bytes(original)
    fleet.cleanup()


def test_invariant_atomic_save_preserves_mode_with_changed_umask(
    source: Repository,
    tmp_path: Path,
) -> None:
    path = source.path / "source-0.c"
    path.chmod(0o755)
    source.commit()
    checkout_fixture(repository=source)
    fleet = new_fleet(source=source, root=tmp_path / "fleet", method=Method.GIT)
    fleet.populate(count=1)
    previous = os.umask(0o077)
    try:
        atomic = fleet.mutate(config=MutationConfig(round_id=0, fraction=1))
    finally:
        os.umask(previous)
    appended = fleet.mutate(
        config=MutationConfig(
            round_id=1,
            fraction=1,
            mode=EditMode.IN_PLACE_APPEND,
        )
    )
    assert atomic.bytes_written > appended.bytes_written
    assert fleet.verify().changed_files == len(fleet.eligible)
    assert asdict(atomic)["selection_sha256"] == appended.selection_sha256
    fleet.cleanup()


@pytest.mark.parametrize("mask", [0o002, 0o022])
@pytest.mark.parametrize("method", list(Method))
def test_invariant_fixture_modes_match_checkout_creation_policy(
    repository: Repository, tmp_path: Path, mask: int, method: Method
) -> None:
    if method is Method.COWTREE and not inspect_path(path=repository.path).supported:
        pytest.skip("native copy-on-write filesystem required")
    previous = os.umask(mask)
    try:
        source = create_source(repository=repository)
        fleet = new_fleet(source=source, root=tmp_path / "fleet", method=method)
        assert fleet.inventory["run"].mode == 0o777 & ~mask
        assert fleet.inventory["source-0.c"].mode == 0o666 & ~mask
        fleet.populate(count=1)
        assert fleet.verify().changed_files == 0
        fleet.cleanup()
    finally:
        os.umask(previous)


def test_invariant_space_ratio_retains_overhead_and_negative_savings() -> None:
    from benchmarks.space_report import Measurement, savings

    rows = [
        Measurement(0, Method.GIT, "pristine", 1100, 100, 1.0),
        Measurement(0, Method.COWTREE, "pristine", 1120, 120, 1.0),
    ]
    assert savings(rows, "pristine", total=False) == pytest.approx([-20.0])
    assert savings(rows, "pristine", total=True) == pytest.approx([-100 * 20 / 1100])


def test_invariant_paired_benchmark_rejects_different_states(tmp_path: Path) -> None:
    from benchmarks.space import Config, compare

    cfg = Config(tmp_path, "a" * 40, tmp_path, 1, 1, 1729, 1)
    records = [{"stage": "verified", "verification": {"state_sha256": "a"}}]
    (tmp_path / "trial-0-git.json").write_text(json.dumps(records))
    records[0]["verification"]["state_sha256"] = "b"
    (tmp_path / "trial-0-cowtree.json").write_text(json.dumps(records))
    with pytest.raises(RuntimeError, match="paired states differ"):
        compare(cfg, 0)


def test_invariant_report_rejects_partial_runs(tmp_path: Path) -> None:
    from benchmarks.space_report import read_measurements

    (tmp_path / "metadata.json").write_text(json.dumps({"config": {"trials": 1}}))
    stages = ("empty", "source", "pristine", "atomic_1pct", "round_2_append")
    records = [
        {"stage": stage, "space": {"used_bytes": index * 100}, "operation_seconds": 1.0}
        for index, stage in enumerate(stages)
    ]
    for method in Method:
        (tmp_path / f"trial-0-{method.value}.json").write_text(json.dumps(records))
    with pytest.raises(ValueError, match="incomplete"):
        read_measurements(root=tmp_path)
