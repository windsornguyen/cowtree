"""Seeded real histories preserve independently modeled source, modes, and caches."""

from pathlib import Path

import pytest

from benchmarks.workspace_history import Config, run

from .test_bootstrap import metadata_binary


@pytest.mark.parametrize("seed", [7, 29])
def test_seeded_managed_history_matches_the_independent_model(tmp_path: Path, seed: int) -> None:
    config = Config(seed=seed, rounds=2, files=8, cache_bytes=32768, binary=metadata_binary())
    receipt = run(root=tmp_path / "history", config=config)
    counts = receipt["counts"]
    assert isinstance(counts, dict)
    assert counts["commit_batch"] == 2
    assert counts["retry_batch"] == 2
    assert counts["lost_replies"] == 1
    assert counts["checkpoint"] == 2
    assert counts["collect"] == 2
    assert counts["edit_symlink"] == 1
    assert counts["edit_executable"] == 1
    assert counts["delete"] == 1
    assert counts["reopen"] == 2
