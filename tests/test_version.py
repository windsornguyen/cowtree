"""Version reporting must identify the installed distribution, not the caller's repository."""

from importlib.metadata import version
from pathlib import Path

from cowtree.version import VersionInfo
from tests.test_cli import run_cli


def test_version_is_available_outside_a_repository(tmp_path: Path) -> None:
    res = run_cli(arguments=["--version"], cwd=tmp_path)
    assert res.returncode == 0, res.stderr
    assert f"cowtree {version('cowtree')}" in res.stdout
    assert not res.stderr


def test_json_version_has_explicit_unknown_provenance(tmp_path: Path) -> None:
    res = run_cli(arguments=["--version", "--json"], cwd=tmp_path)
    assert res.returncode == 0, res.stderr
    info = VersionInfo.model_validate_json(res.stdout)
    assert info.version == version("cowtree")
    assert "revision" in info.model_fields_set
