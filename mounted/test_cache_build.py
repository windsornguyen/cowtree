"""A real make build reuses inherited objects and rebuilds changed source correctly."""

from pathlib import Path

from benchmarks.warm_cache import trial

from .test_bootstrap import metadata_binary


def test_warm_fork_reuses_real_compiler_outputs(tmp_path: Path) -> None:
    result = trial(root=tmp_path / "trial", binary=metadata_binary())
    assert result.ordinary.initial.compiled == ["main.c", "value.c"]
    assert result.ordinary.initial.links == 1
    assert result.cowtree.initial.compiled == []
    assert result.cowtree.initial.links == 0
    assert result.cowtree.inherited_artifacts
    assert result.cowtree.unchanged_object_preserved
    assert result.cowtree.changed.compiled == ["value.c"]
    assert result.cowtree.changed.output == 43
    assert result.ordinary.changed.output == 43
