"""Version queries inspect the configured binary without opening an authority store."""

import os
from pathlib import Path
import sys
from typing import Literal

from pydantic import BaseModel

from cowtree.exec import CommandRunner
from cowtree.tree_types import PathPolicy
from cowtree.version import VersionInfo, WorkspaceVersions
from cowtree.workspace_types import WorkspaceConfig


class Receipt(BaseModel):
    status: Literal["ok"]
    kind: Literal["version"]
    value: WorkspaceVersions


def test_binary_version_is_read_only(tmp_path: Path) -> None:
    binary = Path(__file__).resolve().parents[1] / "target/debug/cowtree-metadata"
    io = CommandRunner()
    res = io.run(argv=[str(binary), "--version", "--json"], cwd=str(tmp_path), check=False)
    assert res.returncode == 0, res.stderr
    assert VersionInfo.model_validate_json(res.stdout).version
    assert list(tmp_path.iterdir()) == []


def test_workspace_reports_its_configured_executable(tmp_path: Path) -> None:
    binary = Path(__file__).resolve().parents[1] / "target/debug/cowtree-metadata"
    cfg = WorkspaceConfig(
        location=tmp_path,
        source=tmp_path,
        git_directory=tmp_path / ".git",
        binary=binary,
        policy=PathPolicy(),
        initial="a" * 64,
        warm_tip="a" * 64,
    )
    (tmp_path / "workspace.json").write_text(cfg.model_dump_json())
    res = CommandRunner().run(
        argv=[
            sys.executable,
            "-c",
            "from cowtree.cli import main; raise SystemExit(main())",
            "workspace",
            "--root",
            str(tmp_path),
            "version",
        ],
        cwd=str(tmp_path),
        env={**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[1] / "src")},
        check=False,
    )
    assert res.returncode == 0, res.stderr
    info = Receipt.model_validate_json(res.stdout).value
    assert info.cli == VersionInfo.installed()
    direct = CommandRunner().run(argv=[str(binary), "--version", "--json"])
    assert info.backend.version == VersionInfo.model_validate_json(direct.stdout).version
    assert info.backend.path == binary
    assert {path.name for path in tmp_path.iterdir()} == {"workspace.json"}
