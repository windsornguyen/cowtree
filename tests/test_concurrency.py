from __future__ import annotations

from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor
from dataclasses import dataclass
import multiprocessing
import os
from pathlib import Path
from typing import Literal

import pytest

from cowtree.core import add_worktree, list_all_worktrees, remove_worktree
from cowtree.errors import CowtreeError, CowtreeErrorCode
from cowtree.types import WorktreeAddRequest

from .conftest import Repository


@dataclass(frozen=True)
class AddOutcome:
    path: Path | None
    error: CowtreeErrorCode | None


def add_concurrently(source: Path, target: Path, branch: str) -> AddOutcome:
    try:
        worktree = add_worktree(
            request=WorktreeAddRequest(path=target, source=source, branch=branch)
        )
    except CowtreeError as error:
        outcome = AddOutcome(path=None, error=error.code)
        return outcome
    outcome = AddOutcome(path=worktree.path, error=None)
    return outcome


def remove_concurrently(source: Path, target: Path) -> None:
    remove_worktree(path=target, source=source)


def list_concurrently(source: Path) -> list[Path]:
    result = [item.path for item in list_all_worktrees(source=source)]
    assert len(result) == len(set(result))
    return result


def worker_count(kind: Literal["threads", "processes"]) -> int:
    count = int(os.environ.get("COWTREE_STRESS_WORKERS", "16" if kind == "threads" else "8"))
    assert 2 <= count <= 64, "COWTREE_STRESS_WORKERS must be between 2 and 64"
    return count


def make_executor(
    kind: Literal["threads", "processes"],
) -> ThreadPoolExecutor | ProcessPoolExecutor:
    if kind == "threads":
        executor = ThreadPoolExecutor(max_workers=worker_count(kind=kind))
        return executor
    processes = ProcessPoolExecutor(
        max_workers=worker_count(kind=kind), mp_context=multiprocessing.get_context("spawn")
    )
    return processes


@pytest.mark.parametrize("kind", ["threads", "processes"])
def test_invariant_concurrent_operations_preserve_registration(
    cow_repository: Repository, tmp_path: Path, kind: Literal["threads", "processes"]
) -> None:
    repo = cow_repository
    targets = [tmp_path / f"parallel-{index}" for index in range(worker_count(kind=kind))]
    with make_executor(kind=kind) as executor:
        adds = [
            executor.submit(add_concurrently, source=repo.path, target=target, branch=target.name)
            for target in targets
        ]
        reads = [executor.submit(list_concurrently, source=repo.path) for _ in range(4)]
        outcomes = [future.result(timeout=60) for future in adds]
        for read in reads:
            read.result(timeout=60)
        assert {outcome.path for outcome in outcomes} == set(targets)
        assert all(outcome.error is None for outcome in outcomes)
        assert repo.registered == {repo.path, *targets}
        for target in targets:
            repo.assert_clean(target=target)
        replacements = [
            tmp_path / f"replacement-{index}" for index in range(worker_count(kind=kind))
        ]
        removes = [
            executor.submit(remove_concurrently, source=repo.path, target=target)
            for target in targets
        ]
        adds = [
            executor.submit(add_concurrently, source=repo.path, target=target, branch=target.name)
            for target in replacements
        ]
        reads = [executor.submit(list_concurrently, source=repo.path) for _ in range(4)]
        for future in removes:
            future.result(timeout=60)
        assert {future.result(timeout=60).path for future in adds} == set(replacements)
        for read in reads:
            read.result(timeout=60)
        assert repo.registered == {repo.path, *replacements}
        removes = [
            executor.submit(remove_concurrently, source=repo.path, target=target)
            for target in replacements
        ]
        for future in removes:
            future.result(timeout=60)
    assert repo.registered == {repo.path}
    assert all(not target.exists() for target in [*targets, *replacements])


@pytest.mark.parametrize("kind", ["threads", "processes"])
def test_invariant_same_target_has_one_owner(
    cow_repository: Repository, tmp_path: Path, kind: Literal["threads", "processes"]
) -> None:
    repo = cow_repository
    target = tmp_path / "shared"
    with make_executor(kind=kind) as executor:
        futures = [
            executor.submit(
                add_concurrently, source=repo.path, target=target, branch=f"owner-{index}"
            )
            for index in range(worker_count(kind=kind))
        ]
        outcomes = [future.result(timeout=60) for future in futures]
    assert sum(outcome.path == target for outcome in outcomes) == 1
    assert sum(outcome.error is not None for outcome in outcomes) == worker_count(kind=kind) - 1
    assert repo.registered == {repo.path, target}
    branches = repo.git(
        "for-each-ref", "--format=%(refname)", "refs/heads/owner-*"
    ).stdout.splitlines()
    assert len(branches) == 1
    repo.assert_clean(target=target)
